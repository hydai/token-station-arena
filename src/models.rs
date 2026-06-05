use anyhow::{bail, Result};

use crate::types::ModelConfig;

/// Selects which models to run.
///
/// `selector` is either `"all"` (every enabled model) or a comma-separated list
/// of model ids or provider model ids. Only enabled models are eligible; a
/// selector naming an unknown or disabled model is an error. The returned order
/// follows the input `models` order, not the selector order.
pub fn select_models(models: &[ModelConfig], selector: &str) -> Result<Vec<ModelConfig>> {
    let enabled: Vec<ModelConfig> = models.iter().filter(|m| m.enabled).cloned().collect();
    if selector == "all" {
        return Ok(enabled);
    }

    let requested: Vec<&str> = selector
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();

    let selected: Vec<ModelConfig> = enabled
        .into_iter()
        .filter(|m| requested.contains(&m.id.as_str()) || requested.contains(&m.model.as_str()))
        .collect();

    let mut found = std::collections::HashSet::new();
    for m in &selected {
        found.insert(m.id.as_str());
        found.insert(m.model.as_str());
    }
    let missing: Vec<&str> = requested
        .iter()
        .copied()
        .filter(|r| !found.contains(r))
        .collect();
    if !missing.is_empty() {
        bail!("Unknown or disabled model id(s): {}", missing.join(", "));
    }

    Ok(selected)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str, provider_model: &str, enabled: bool) -> ModelConfig {
        ModelConfig {
            id: id.to_string(),
            display_name: id.to_uppercase(),
            provider: "models.bytefuture.ai".to_string(),
            model: provider_model.to_string(),
            claude_model_strategy: "custom-model-option".to_string(),
            enabled,
        }
    }

    fn sample() -> Vec<ModelConfig> {
        vec![
            model("a", "prov/a", true),
            model("b", "prov/b", false),
            model("c", "prov/c", true),
        ]
    }

    fn ids(models: &[ModelConfig]) -> Vec<&str> {
        models.iter().map(|m| m.id.as_str()).collect()
    }

    #[test]
    fn all_returns_only_enabled_models_in_input_order() {
        let selected = select_models(&sample(), "all").unwrap();
        assert_eq!(ids(&selected), ["a", "c"]);
    }

    #[test]
    fn selector_matches_id_or_provider_model_id_preserving_input_order() {
        let selected = select_models(&sample(), "c, prov/a").unwrap();
        assert_eq!(ids(&selected), ["a", "c"]);
    }

    #[test]
    fn selecting_a_disabled_model_is_an_error() {
        let err = select_models(&sample(), "b").unwrap_err();
        assert!(err.to_string().contains('b'));
    }

    #[test]
    fn selecting_an_unknown_model_is_an_error() {
        let err = select_models(&sample(), "nope").unwrap_err();
        assert!(err.to_string().contains("nope"));
    }
}
