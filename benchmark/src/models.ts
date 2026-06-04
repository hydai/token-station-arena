import type { ModelConfig } from "./types.ts";

export function selectModels(models: ModelConfig[], selector = "all"): ModelConfig[] {
  const enabled = models.filter((model) => model.enabled);
  if (selector === "all") return enabled;

  const requested = new Set(selector.split(",").map((value) => value.trim()).filter(Boolean));
  const selected = enabled.filter((model) => requested.has(model.id) || requested.has(model.model));
  const found = new Set<string>();
  for (const model of selected) {
    found.add(model.id);
    found.add(model.model);
  }

  const missing = [...requested].filter((modelId) => !found.has(modelId));
  if (missing.length > 0) {
    throw new Error(`Unknown or disabled model id(s): ${missing.join(", ")}`);
  }

  return selected;
}
