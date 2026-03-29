import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open, save } from "@tauri-apps/plugin-dialog";
import yaml from "js-yaml";
import { ClassBrowser } from "./components/ClassBrowser";
import { ObjectEditor } from "./components/ObjectEditor";
import { OutputsEditor } from "./components/OutputsEditor";
import { SimulationPanel } from "./components/SimulationPanel";
import { ResultsView } from "./components/ResultsView";
import { NetworkView } from "./components/NetworkView";
import { ParametricView } from "./components/ParametricView";
import { HelpDialog } from "./components/HelpDialog";
import { parseSchema } from "./lib/schema";
import type { ClassInfo } from "./lib/schema";
import type { ParsedCsv } from "./lib/csv";
import { parseCsv } from "./lib/csv";
import { loadSettings, saveSettings, type UnitSystem } from "./lib/units";
import logoSvg from "./assets/logo.svg";
import "./App.css";

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Model = Record<string, any>;

type ViewMode = "edit" | "network" | "charts" | "parametric";

const ZoneTag = new yaml.Type("!zone", {
  kind: "scalar",
  construct(data: string) {
    return { zone: data };
  },
  predicate(obj: unknown) {
    return (
      typeof obj === "object" &&
      obj !== null &&
      "zone" in obj &&
      Object.keys(obj as object).length === 1
    );
  },
  represent(obj: unknown) {
    return (obj as { zone: string }).zone;
  },
});

const OPENBSE_SCHEMA = yaml.DEFAULT_SCHEMA.extend([ZoneTag]);

function serializeYaml(model: Model): string {
  const clean = JSON.parse(JSON.stringify(model));
  return yaml.dump(clean, {
    indent: 2,
    lineWidth: 120,
    noRefs: true,
    sortKeys: false,
    quotingType: '"',
    forceQuotes: false,
    schema: OPENBSE_SCHEMA,
  });
}

function yieldToUI(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

export interface ResultCase {
  name: string;
  path: string;
  parsed: ParsedCsv | null;
}

function App() {
  const [classes, setClasses] = useState<ClassInfo[]>([]);
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [model, setModel] = useState<Model>({});
  const [filePath, setFilePath] = useState<string | null>(null);
  const [dirty, setDirty] = useState(false);
  const [viewMode, setViewMode] = useState<ViewMode>("edit");
  const [helpOpen, setHelpOpen] = useState(false);
  const [unitSystem, setUnitSystem] = useState<UnitSystem>(() => loadSettings().unitSystem);

  const toggleUnitSystem = useCallback(() => {
    setUnitSystem((prev) => {
      const next = prev === "SI" ? "IP" : "SI";
      saveSettings({ unitSystem: next });
      return next;
    });
  }, []);

  // ===== Shared results state =====
  const [resultsCases, setResultsCases] = useState<ResultCase[]>([]);
  const [resultsActiveIdx, setResultsActiveIdx] = useState(0);
  const [resultsLoading, setResultsLoading] = useState(false);

  // ===== Shared variable selection (global across Network + Charts) =====
  const [selectedVarIndices, setSelectedVarIndices] = useState<Set<number>>(
    new Set()
  );

  const toggleVariable = useCallback((idx: number) => {
    setSelectedVarIndices((prev) => {
      const next = new Set(prev);
      if (next.has(idx)) next.delete(idx);
      else next.add(idx);
      return next;
    });
  }, []);

  const setVariables = useCallback((indices: Set<number>) => {
    setSelectedVarIndices(indices);
  }, []);

  const clearAllVariables = useCallback(() => {
    setSelectedVarIndices(new Set());
  }, []);

  useEffect(() => {
    async function loadSchema() {
      try {
        const rawSchema = await invoke<Record<string, unknown>>("load_schema");
        const parsed = parseSchema(rawSchema);
        setClasses(parsed);
        setError(null);
      } catch (e) {
        setError(String(e));
      } finally {
        setLoading(false);
      }
    }
    loadSchema();
  }, []);

  const updateModel = useCallback(
    (key: string, value: unknown) => {
      setModel((prev) => ({ ...prev, [key]: value }));
      setDirty(true);
    },
    []
  );

  // ===== File I/O =====

  const handleNew = useCallback(() => {
    setModel({});
    setFilePath(null);
    setDirty(false);
  }, []);

  const handleOpen = useCallback(async () => {
    const selected = await open({
      title: "Open OpenBSE Model",
      multiple: false,
      directory: false,
      filters: [{ name: "YAML", extensions: ["yaml", "yml"] }],
    });
    if (!selected) return;
    try {
      const path = selected as string;
      const contents = await invoke<string>("read_yaml_file", { path });
      const parsed = parseYaml(contents);
      setModel(parsed);
      setFilePath(path);
      setDirty(false);
    } catch (e) {
      setError(`Failed to open file: ${e}`);
    }
  }, []);

  const handleSave = useCallback(async () => {
    let path = filePath;
    if (!path) {
      const selected = await save({
        title: "Save OpenBSE Model",
        filters: [{ name: "YAML", extensions: ["yaml", "yml"] }],
        defaultPath: "model.yaml",
      });
      if (!selected) return;
      path = selected;
    }
    try {
      const yamlStr = serializeYaml(model);
      await invoke("write_yaml_file", { path, contents: yamlStr });
      setFilePath(path);
      setDirty(false);
    } catch (e) {
      setError(`Failed to save: ${e}`);
    }
  }, [filePath, model]);

  const handleSaveAs = useCallback(async () => {
    const selected = await save({
      title: "Save OpenBSE Model As",
      filters: [{ name: "YAML", extensions: ["yaml", "yml"] }],
      defaultPath: filePath || "model.yaml",
    });
    if (!selected) return;
    try {
      const yamlStr = serializeYaml(model);
      await invoke("write_yaml_file", { path: selected, contents: yamlStr });
      setFilePath(selected);
      setDirty(false);
    } catch (e) {
      setError(`Failed to save: ${e}`);
    }
  }, [filePath, model]);

  // ===== Instance Management =====

  const getInstances = useCallback(
    (key: string): unknown[] => {
      const val = model[key];
      return Array.isArray(val) ? val : [];
    },
    [model]
  );

  const instanceCounts: Record<string, number> = {};
  for (const cls of classes) {
    if (cls.isArray) {
      instanceCounts[cls.key] = getInstances(cls.key).length;
    }
  }

  const selectedClass = classes.find((c) => c.key === selectedKey) ?? null;

  // ===== Results loading (shared) =====
  const loadAndParseCsv = useCallback(
    async (path: string): Promise<ResultCase | null> => {
      try {
        const contents = await invoke<string>("read_yaml_file", { path });
        await yieldToUI();
        const parsed = parseCsv(contents);
        const name = path.split("/").pop() ?? path;
        return { name, path, parsed };
      } catch (e) {
        setError(`Failed to load ${path}: ${e}`);
        return null;
      }
    },
    []
  );

  // Lazy-load CSV when active case changes (works from any view)
  useEffect(() => {
    if (resultsCases.length === 0) return;
    const c = resultsCases[resultsActiveIdx];
    if (!c || c.parsed) return;
    let cancelled = false;
    (async () => {
      setResultsLoading(true);
      const result = await loadAndParseCsv(c.path);
      if (cancelled) return;
      if (result) {
        setResultsCases((prev) => {
          const updated = [...prev];
          updated[resultsActiveIdx] = result;
          return updated;
        });
      }
      setResultsLoading(false);
    })();
    return () => { cancelled = true; };
  }, [resultsActiveIdx, resultsCases, loadAndParseCsv]);

  const handleLoadResultsFile = useCallback(async () => {
    const selected = await open({
      title: "Open Results CSV",
      multiple: true,
      directory: false,
      filters: [
        { name: "CSV Results", extensions: ["csv"] },
        { name: "All Files", extensions: ["*"] },
      ],
    });
    if (!selected) return;
    setResultsLoading(true);
    const paths = Array.isArray(selected) ? selected : [selected];
    const newCases: ResultCase[] = [];
    for (const p of paths) {
      const result = await loadAndParseCsv(p as string);
      if (result) newCases.push(result);
    }
    if (newCases.length > 0) {
      setResultsCases((prev) => [...prev, ...newCases]);
      setResultsActiveIdx(resultsCases.length);
      setSelectedVarIndices(new Set());
    }
    setResultsLoading(false);
    if (viewMode !== "charts") setViewMode("charts");
  }, [loadAndParseCsv, resultsCases.length, viewMode]);

  const handleOpenFolder = useCallback(async () => {
    const selected = await open({
      title: "Open Project Folder",
      multiple: false,
      directory: true,
    });
    if (!selected) return;
    try {
      const result = await invoke<{ yaml_files: string[]; csv_files: string[] }>(
        "scan_project_folder",
        { dir: selected as string }
      );
      if (result.yaml_files.length > 0) {
        const yamlPath = result.yaml_files[0];
        try {
          const contents = await invoke<string>("read_yaml_file", { path: yamlPath });
          const parsed = parseYaml(contents);
          setModel(parsed);
          setFilePath(yamlPath);
          setDirty(false);
        } catch (e) {
          setError(`Failed to load model: ${e}`);
        }
      }
      if (result.csv_files.length > 0) {
        setResultsLoading(true);
        // Sort: prefer main *_results.csv, then others
        const sorted = [...result.csv_files].sort((a, b) => {
          const aName = a.split("/").pop() ?? a;
          const bName = b.split("/").pop() ?? b;
          // Main results file (ends with _results.csv but not zone_, surface_, hvac_, sizing, summary)
          const isMain = (n: string) => n.endsWith("_results.csv") && !n.includes("zone_") && !n.includes("surface_") && !n.includes("hvac_") && !n.includes("sizing") && !n.includes("summary");
          const aMain = isMain(aName);
          const bMain = isMain(bName);
          if (aMain && !bMain) return -1;
          if (!aMain && bMain) return 1;
          return aName.localeCompare(bName);
        });
        const lazyCases: ResultCase[] = sorted.map((f) => ({
          name: f.split("/").pop() ?? f,
          path: f,
          parsed: null,
        }));
        const firstResult = await loadAndParseCsv(sorted[0]);
        if (firstResult) {
          lazyCases[0] = firstResult;
        } else if (sorted.length > 1) {
          // First file failed (maybe too large), try the next
          const fallback = await loadAndParseCsv(sorted[1]);
          if (fallback) lazyCases[1] = fallback;
          setResultsActiveIdx(1);
        }
        setResultsCases(lazyCases);
        setResultsActiveIdx(0);
        setSelectedVarIndices(new Set());
        setResultsLoading(false);
      }
      if (result.yaml_files.length === 0 && result.csv_files.length === 0) {
        setError("No YAML or CSV files found in the selected folder.");
      }
    } catch (e) {
      setError(`Failed to scan folder: ${e}`);
    }
  }, [loadAndParseCsv]);

  // ===== Simulation results auto-load =====
  const handleSimulationComplete = useCallback(
    async (csvPath: string) => {
      setResultsLoading(true);

      // Scan the output folder for all CSV files the engine produced
      const dir = csvPath.substring(0, csvPath.lastIndexOf("/"));
      try {
        const scanResult = await invoke<{ yaml_files: string[]; csv_files: string[] }>(
          "scan_project_folder",
          { dir }
        );
        if (scanResult.csv_files.length > 1) {
          // Multiple CSVs — sort with main results first, load the primary one
          const sorted = [...scanResult.csv_files].sort((a, b) => {
            const aName = a.split("/").pop() ?? a;
            const bName = b.split("/").pop() ?? b;
            const isMain = (n: string) => n.endsWith("_results.csv") && !n.includes("zone_") && !n.includes("surface_") && !n.includes("hvac_") && !n.includes("sizing") && !n.includes("summary");
            const aMain = isMain(aName);
            const bMain = isMain(bName);
            if (aMain && !bMain) return -1;
            if (!aMain && bMain) return 1;
            return aName.localeCompare(bName);
          });
          const lazyCases: ResultCase[] = sorted.map((f) => ({
            name: f.split("/").pop() ?? f,
            path: f,
            parsed: null,
          }));
          const firstResult = await loadAndParseCsv(sorted[0]);
          if (firstResult) lazyCases[0] = firstResult;
          setResultsCases(lazyCases);
          setResultsActiveIdx(0);
          setSelectedVarIndices(new Set());
          setResultsLoading(false);
          setViewMode("charts");
          return;
        }
      } catch {
        // Folder scan failed — fall back to loading just the single CSV
      }

      // Fallback: load just the reported CSV
      const result = await loadAndParseCsv(csvPath);
      if (result) {
        setResultsCases([result]);
        setResultsActiveIdx(0);
        setSelectedVarIndices(new Set());
      }
      setResultsLoading(false);
      setViewMode("charts");
    },
    [loadAndParseCsv]
  );

  // ===== Menu events =====
  useEffect(() => {
    const unlisten = listen<string>("menu-action", (event) => {
      switch (event.payload) {
        case "file_new": handleNew(); break;
        case "file_open": handleOpen(); break;
        case "file_save": handleSave(); break;
        case "file_save_as": handleSaveAs(); break;
        case "help_usage": setHelpOpen(true); break;
      }
    });
    return () => { unlisten.then((f) => f()); };
  }, [handleOpen, handleSave, handleSaveAs, handleNew]);

  if (loading) {
    return <div className="app-loading"><p>Loading schema...</p></div>;
  }

  if (error) {
    return (
      <div className="app-error">
        <h2>Error</h2>
        <pre>{error}</pre>
        <button className="btn-secondary" onClick={() => setError(null)}>Dismiss</button>
      </div>
    );
  }

  const fileName = filePath ? filePath.split("/").pop() || "model.yaml" : "Untitled";
  const activeResult = resultsCases[resultsActiveIdx] ?? null;
  const resultsName = activeResult?.name ?? null;
  const activeParsed = activeResult?.parsed ?? null;

  return (
    <div className="app">
      <header className="app-header">
        <img src={logoSvg} alt="OpenBSE Workbench" className="header-logo" />
        <div className="header-open-actions">
          <button className="btn-header" onClick={handleOpenFolder}
            title="Open project folder (auto-finds YAML model + CSV results)">
            Open Folder
          </button>
          <button className="btn-header" onClick={handleOpen}
            title="Open YAML model file">
            Open YAML
          </button>
          <button className="btn-header" onClick={handleLoadResultsFile}
            title="Open results CSV file">
            Open CSV
          </button>
        </div>
        <span className="header-filename">
          {fileName}{dirty && " *"}
        </span>
        <nav className="view-mode-tabs">
          <button className={`view-tab ${viewMode === "edit" ? "active" : ""}`}
            onClick={() => setViewMode("edit")}>&#9998; Edit</button>
          <button className={`view-tab ${viewMode === "network" ? "active" : ""}`}
            onClick={() => setViewMode("network")}>&#9741; Network</button>
          <button className={`view-tab ${viewMode === "charts" ? "active" : ""}`}
            onClick={() => setViewMode("charts")}>&#9776; Charts</button>
          <button className={`view-tab ${viewMode === "parametric" ? "active" : ""}`}
            onClick={() => setViewMode("parametric")}>&#8644; Parametric</button>
        </nav>
        <div className="header-results-status">
          {resultsName ? (
            <span className="results-loaded-badge" title={activeResult?.path ?? ""}>
              {resultsName}
              {resultsCases.length > 1 && ` (+${resultsCases.length - 1})`}
            </span>
          ) : (
            <span className="results-none-badge">No results</span>
          )}
          {selectedVarIndices.size > 0 && (
            <button className="btn-header btn-small" onClick={clearAllVariables}
              title="Clear all selected variables">
              Clear {selectedVarIndices.size} vars
            </button>
          )}
        </div>
        <button
          className={`btn-header btn-unit-toggle ${unitSystem === "IP" ? "active" : ""}`}
          onClick={toggleUnitSystem}
          title={`Units: ${unitSystem} (click to switch to ${unitSystem === "SI" ? "IP" : "SI"})`}
        >
          {unitSystem}
        </button>
        <button className="btn-header btn-help" onClick={() => setHelpOpen(true)}
          title="Help &amp; usage guide">?</button>
      </header>

      <HelpDialog open={helpOpen} onClose={() => setHelpOpen(false)} />

      {viewMode === "edit" ? (
        <>
          <div className="app-body">
            <ClassBrowser
              classes={classes}
              selectedKey={selectedKey}
              onSelect={setSelectedKey}
              instanceCounts={instanceCounts}
            />
            <div className="editor-panel">
              {selectedClass ? (
                selectedClass.key === "outputs" ? (
                  // Dedicated output variable picker
                  <OutputsEditor
                    instances={getInstances("outputs")}
                    onUpdate={(updated) => updateModel("outputs", updated.length > 0 ? updated : undefined)}
                  />
                ) : selectedClass.isArray ? (
                  <ObjectEditor
                    classInfo={selectedClass}
                    instances={getInstances(selectedClass.key) as Record<string, unknown>[]}
                    model={model}
                    onUpdate={(idx, updated) => {
                      const arr = [...getInstances(selectedClass.key)];
                      arr[idx] = updated;
                      updateModel(selectedClass.key, arr);
                    }}
                    onAdd={() => {
                      const arr = [...getInstances(selectedClass.key)];
                      const newObj: Record<string, unknown> = {};
                      const schema = selectedClass.itemSchema;
                      if (schema.properties) {
                        for (const f of Object.values(schema.properties)) {
                          if (f.constValue !== undefined) newObj[f.name] = f.constValue;
                          else if (f.required && f.default !== undefined) newObj[f.name] = f.default;
                          else if (f.required && f.type === "string") newObj[f.name] = "";
                        }
                      }
                      arr.push(newObj);
                      updateModel(selectedClass.key, arr);
                    }}
                    onDuplicate={(idx) => {
                      const arr = [...getInstances(selectedClass.key)];
                      const dup = JSON.parse(JSON.stringify(arr[idx]));
                      if (dup.name) dup.name = dup.name + " (copy)";
                      arr.splice(idx + 1, 0, dup);
                      updateModel(selectedClass.key, arr);
                    }}
                    onDelete={(idx) => {
                      const arr = [...getInstances(selectedClass.key)];
                      arr.splice(idx, 1);
                      updateModel(selectedClass.key, arr.length > 0 ? arr : undefined);
                    }}
                    onMove={(idx, direction) => {
                      const arr = [...getInstances(selectedClass.key)];
                      const newIdx = direction === "up" ? idx - 1 : idx + 1;
                      if (newIdx < 0 || newIdx >= arr.length) return;
                      [arr[idx], arr[newIdx]] = [arr[newIdx], arr[idx]];
                      updateModel(selectedClass.key, arr);
                    }}
                  />
                ) : (
                  <ObjectEditor
                    classInfo={selectedClass}
                    instances={
                      model[selectedClass.key] !== undefined
                        ? [model[selectedClass.key]] : [{}]
                    }
                    model={model}
                    onUpdate={(_idx, updated) => updateModel(selectedClass.key, updated)}
                    onAdd={() => {}} onDuplicate={() => {}}
                    onDelete={() => {}} onMove={() => {}}
                  />
                )
              ) : (
                <div className="empty-state">
                  <p>Select an object class from the left panel to begin editing.</p>
                  <p className="hint">Open an existing model with <kbd>Cmd+O</kbd> or start adding objects to a new model.</p>
                </div>
              )}
            </div>
          </div>
          <SimulationPanel
            modelPath={filePath} dirty={dirty}
            onSave={handleSave} onSimulationComplete={handleSimulationComplete}
          />
        </>
      ) : viewMode === "network" ? (
        <NetworkView
          model={model}
          parsedCsv={activeParsed}
          selectedVarIndices={selectedVarIndices}
          onToggleVariable={toggleVariable}
          onSetVariables={setVariables}
          onClearVariables={clearAllVariables}
          resultsCases={resultsCases}
          activeCaseIdx={resultsActiveIdx}
          onCaseChange={(idx) => { setResultsActiveIdx(idx); setSelectedVarIndices(new Set()); }}
          resultsLoading={resultsLoading}
          unitSystem={unitSystem}
        />
      ) : viewMode === "parametric" ? (
        <ParametricView
          cases={resultsCases}
          model={model}
          unitSystem={unitSystem}
          loadAndParseCsv={loadAndParseCsv}
          setCases={setResultsCases}
        />
      ) : (
        <ResultsView
          cases={resultsCases}
          activeCaseIdx={resultsActiveIdx}
          setActiveCaseIdx={setResultsActiveIdx}
          loading={resultsLoading}
          selectedVarIndices={selectedVarIndices}
          onToggleVariable={toggleVariable}
          onClearVariables={clearAllVariables}
          unitSystem={unitSystem}
        />
      )}
    </div>
  );
}

function parseYaml(yamlStr: string): Model {
  const result = yaml.load(yamlStr, { schema: OPENBSE_SCHEMA });
  if (typeof result !== "object" || result === null) return {};
  return result as Model;
}

export default App;
