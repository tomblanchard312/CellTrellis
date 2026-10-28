"""Nanshe's trusted Python runner. Workbook calls use private JSON lines."""
import builtins
import io
import json
import sys

_wire_in, _wire_out = sys.stdin, sys.stdout


def emit(event):
    _wire_out.write(json.dumps(event, ensure_ascii=False, allow_nan=False) + "\n")
    _wire_out.flush()


def read_frame():
    line = _wire_in.readline(256 * 1024 + 1)
    if len(line) > 256 * 1024 or not line.endswith("\n"):
        raise ValueError("Host disconnected or frame exceeds 256 KiB")
    return json.loads(line)


class Workbook:
    """Zero-based sheet indexes or sheet names; rectangular A1 ranges."""
    def __init__(self):
        self.calls = 0

    def execute(self, command, **params):
        self.calls += 1
        if self.calls > 1000:
            raise ValueError("Script exceeded 1,000 engine calls")
        emit({"type": "command", "command": command, "params": params})
        reply = read_frame()
        if "error" in reply:
            raise ValueError(reply["error"])
        return reply.get("result")

    def read(self, address, sheet=0, formulas=False):
        return self.execute("sheet.read", range=address, sheet=sheet, formulas=formulas)["values"]

    def write(self, address, rows, sheet=0, formulas=False):
        # Value writes are literal, including text beginning with = or a quote.
        values = [["'" + value if isinstance(value, str) and not formulas else value
                   for value in row] for row in rows]
        return self.execute("range.setValues", range=address, sheet=sheet,
                            values=values, parse=not formulas)


class Output(io.TextIOBase):
    def write(self, text):
        if text:
            if len(text.encode("utf-8")) > 64 * 1024:
                raise ValueError("Script output exceeds 64 KiB")
            emit({"type": "output", "text": text})
        return len(text)

    def flush(self):
        _wire_out.flush()


try:
    request = read_frame()
    source = request["source"]
    if len(source.encode("utf-8")) > 128 * 1024:
        raise ValueError("Source exceeds 128 KiB")
    sys.stdout = sys.stderr = Output()
    scope = {"nanshe": Workbook(), "__name__": "__main__", "__builtins__": builtins}
    exec(compile(source, "<Nanshe Python>", "exec"), scope)
    emit({"type": "done"})
except BaseException as error:
    emit({"type": "error", "message": f"{type(error).__name__}: {error}"[:8000]})
    sys.exit(1)
