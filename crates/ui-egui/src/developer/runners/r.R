# Trusted R scripts use the same private JSON-line workbook bridge as VBA and Python.
wire_in <- file("stdin", open = "r")
wire_out <- stdout()
emit <- function(event) {
  cat(jsonlite::toJSON(event, auto_unbox = TRUE, null = "null", digits = NA), "\n", file = wire_out, sep = "")
  flush(wire_out)
}
read_frame <- function() {
  line <- readLines(wire_in, n = 1L, warn = FALSE)
  if (length(line) != 1L || nchar(line, type = "bytes") > 256L * 1024L) stop("Host disconnected or frame exceeds 256 KiB")
  jsonlite::fromJSON(line, simplifyVector = FALSE)
}
calls <- 0L
execute <- function(command, params) {
  calls <<- calls + 1L
  if (calls > 1000L) stop("Script exceeded 1,000 engine calls")
  emit(list(type = "command", command = command, params = params))
  reply <- read_frame()
  if (!is.null(reply$error)) stop(reply$error)
  reply$result
}
sheet_read <- function(address, sheet = 0L, formulas = FALSE) {
  execute("sheet.read", list(range = address, sheet = sheet, formulas = formulas))$values
}
sheet_write <- function(address, values, sheet = 0L, formulas = FALSE) {
  if (is.data.frame(values)) {
    values <- lapply(seq_len(nrow(values)), function(i) unname(as.list(values[i, , drop = FALSE])))
  } else if (is.matrix(values)) {
    values <- lapply(seq_len(nrow(values)), function(i) as.list(values[i, ]))
  } else if (!is.list(values) || any(!vapply(values, is.list, logical(1)))) {
    stop("values must be a matrix, data.frame, or list of row lists")
  }
  literal <- function(v) {
    if (length(v) > 1L || (length(v) == 1L && is.na(v))) stop("Cells must be finite scalars or NULL, not NA")
    if (is.numeric(v) && any(!is.finite(v))) stop("Cells must be finite numbers")
    if (is.character(v) && !formulas) paste0("'", v) else v
  }
  values <- lapply(values, function(row) lapply(row, literal))
  execute("range.setValues", list(range = address, sheet = sheet, values = values, parse = !formulas))
}
tryCatch({
  if (!requireNamespace("jsonlite", quietly = TRUE)) stop("Install the R package jsonlite before running R scripts")
  request <- read_frame()
  if (nchar(request$source, type = "bytes") > 128L * 1024L) stop("Source exceeds 128 KiB")
  scope <- new.env(parent = globalenv())
  scope$sheet_read <- sheet_read
  scope$sheet_write <- sheet_write
  scope$print <- function(x, ...) {
    text <- paste(capture.output(base::print(x, ...)), collapse = "\n")
    if (nchar(text, type = "bytes") > 64L * 1024L) stop("Script output exceeds 64 KiB")
    emit(list(type = "output", text = paste0(text, "\n")))
    invisible(x)
  }
  scope$cat <- function(..., sep = " ") {
    text <- paste(..., sep = sep, collapse = sep)
    emit(list(type = "output", text = text))
    invisible(NULL)
  }
  eval(parse(text = request$source), envir = scope)
  emit(list(type = "done"))
}, error = function(error) {
  if (requireNamespace("jsonlite", quietly = TRUE)) {
    emit(list(type = "error", message = substr(conditionMessage(error), 1L, 8000L)))
  } else {
    cat('{"type":"error","message":"R requires the jsonlite package; install it before running scripts"}\n', file = wire_out)
  }
  quit(status = 1L)
})
