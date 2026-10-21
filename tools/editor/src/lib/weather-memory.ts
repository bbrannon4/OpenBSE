// Remembers the weather file chosen for each model, so the selection survives
// tab switches and app restarts. Stored per model (not globally) so a model
// whose YAML `weather_files` is correct is never silently overridden by a
// weather file picked for a different model.

const WEATHER_KEY = "openbse-workbench-weather-by-model";

function normalizeModelPath(modelPath: string): string {
  const p = modelPath.replace(/\\/g, "/");
  // Windows paths are case-insensitive
  return /^[a-zA-Z]:\//.test(p) ? p.toLowerCase() : p;
}

function loadAll(): Record<string, string> {
  try {
    const raw = localStorage.getItem(WEATHER_KEY);
    if (raw) {
      const parsed = JSON.parse(raw);
      if (parsed && typeof parsed === "object") return parsed;
    }
  } catch {
    // ignore
  }
  return {};
}

export function getRememberedWeather(modelPath: string | null): string | null {
  if (!modelPath) return null;
  return loadAll()[normalizeModelPath(modelPath)] ?? null;
}

export function rememberWeather(modelPath: string, weatherPath: string | null): void {
  const all = loadAll();
  const key = normalizeModelPath(modelPath);
  if (weatherPath) {
    all[key] = weatherPath;
  } else {
    delete all[key];
  }
  try {
    localStorage.setItem(WEATHER_KEY, JSON.stringify(all));
  } catch {
    // ignore
  }
}
