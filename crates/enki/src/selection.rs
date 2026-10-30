/// Select an installed Enki text model for a task without exposing model details in the UI.
/// shortcut: until Enki advertises task capabilities, rank registered text model families.
pub fn select_text_model(models: &[String], task: &str) -> Option<String> {
    models
        .iter()
        .filter_map(|id| {
            let name = id.to_ascii_lowercase();
            if ["embed", "rerank", "bge-", "mmproj", "whisper", "clip", "vae", "flux", "image", "diffusion", "sd-turbo", "sdxl", "stable-"]
                .iter()
                .any(|word| name.contains(word))
            {
                return None;
            }
            let code = ["coder", "codestral", "starcoder", "codegemma"].iter().any(|word| name.contains(word));
            let coding = matches!(task, "code" | "formula");
            let score = if coding && code {
                1000
            } else if coding && name.contains("math") {
                800
            } else if name.contains("phi-4") || name.contains("phi4") {
                700
            } else if code {
                100
            } else if name.contains("instruct") {
                500
            } else if name.contains("chat") {
                400
            } else {
                200
            };
            Some((score, id))
        })
        .max_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)))
        .map(|(_, id)| id.clone())
}
#[cfg(test)]
mod task_selection_tests {
    use super::*;
    #[test]
    fn task_routing_filters_non_text_models_and_prefers_a_code_specialist_for_code() {
        let models = [
            "bge-small",
            "qwen-image-2512",
            "flux-2",
            "z-image-turbo",
            "qwen-coder-instruct",
            "microsoft_Phi-4-mini-instruct",
            "qwen2.5-0.5b-instruct",
        ]
        .map(String::from);
        assert_eq!(select_text_model(&models, "writing").as_deref(), Some("microsoft_Phi-4-mini-instruct"));
        assert_eq!(select_text_model(&models, "code").as_deref(), Some("qwen-coder-instruct"));
        assert_eq!(select_text_model(&models, "formula").as_deref(), Some("qwen-coder-instruct"));
        assert!(select_text_model(&["qwen-image".into(), "bge-small".into()], "writing").is_none());
    }
}
