import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import yaml from "js-yaml";
import { ClassBrowser } from "./components/ClassBrowser";
import { ObjectEditor } from "./components/ObjectEditor";
import { parseSchema } from "./lib/schema";
import type { ClassInfo } from "./lib/schema";
import "./App.css";

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Model = Record<string, any>;

/**
 * Custom YAML type for the !zone tag used by serde_yaml for
 * BoundaryCondition::Zone(String). Converts !zone "name" <-> {zone: "name"}.
 */
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

/** Serialize model to YAML using js-yaml with clean formatting */
function serializeYaml(model: Model): string {
  // Strip undefined values before dumping
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

function App() {
  const [classes, setClasses] = useState<ClassInfo[]>([]);
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [model, setModel] = useState<Model>({});
  const [filePath, setFilePath] = useState<string | null>(null);
  const [dirty, setDirty] = useState(false);

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

  // ===== Keyboard shortcuts =====
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if ((e.metaKey || e.ctrlKey) && e.key === "o") {
        e.preventDefault();
        handleOpen();
      }
      if ((e.metaKey || e.ctrlKey) && e.key === "s") {
        e.preventDefault();
        if (e.shiftKey) {
          handleSaveAs();
        } else {
          handleSave();
        }
      }
      if ((e.metaKey || e.ctrlKey) && e.key === "n") {
        e.preventDefault();
        handleNew();
      }
    }
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [handleOpen, handleSave, handleSaveAs, handleNew]);

  if (loading) {
    return (
      <div className="app-loading">
        <p>Loading schema...</p>
      </div>
    );
  }

  if (error) {
    return (
      <div className="app-error">
        <h2>Error</h2>
        <pre>{error}</pre>
        <button className="btn-secondary" onClick={() => setError(null)}>
          Dismiss
        </button>
      </div>
    );
  }

  const fileName = filePath
    ? filePath.split("/").pop() || "model.yaml"
    : "Untitled";

  return (
    <div className="app">
      <header className="app-header">
        <h1>OpenBSE Editor</h1>
        <span className="header-filename">
          {fileName}
          {dirty && " *"}
        </span>
        <div className="header-actions">
          <button className="btn-header" onClick={handleNew} title="New (Cmd+N)">
            New
          </button>
          <button
            className="btn-header"
            onClick={handleOpen}
            title="Open (Cmd+O)"
          >
            Open
          </button>
          <button
            className="btn-header"
            onClick={handleSave}
            title="Save (Cmd+S)"
            disabled={!dirty && filePath !== null}
          >
            Save
          </button>
          <button
            className="btn-header"
            onClick={handleSaveAs}
            title="Save As (Cmd+Shift+S)"
          >
            Save As
          </button>
        </div>
      </header>
      <div className="app-body">
        <ClassBrowser
          classes={classes}
          selectedKey={selectedKey}
          onSelect={setSelectedKey}
          instanceCounts={instanceCounts}
        />
        <div className="editor-panel">
          {selectedClass ? (
            selectedClass.isArray ? (
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
                  // Pre-populate required fields with defaults
                  const schema = selectedClass.itemSchema;
                  if (schema.properties) {
                    for (const f of Object.values(schema.properties)) {
                      if (f.constValue !== undefined) {
                        newObj[f.name] = f.constValue;
                      } else if (f.required && f.default !== undefined) {
                        newObj[f.name] = f.default;
                      } else if (f.required && f.type === "string") {
                        newObj[f.name] = "";
                      }
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
                    ? [model[selectedClass.key]]
                    : [{}]
                }
                model={model}
                onUpdate={(_idx, updated) => {
                  updateModel(selectedClass.key, updated);
                }}
                onAdd={() => {}}
                onDuplicate={() => {}}
                onDelete={() => {}}
                onMove={() => {}}
              />
            )
          ) : (
            <div className="empty-state">
              <p>Select an object class from the left panel to begin editing.</p>
              <p className="hint">
                Open an existing model with <kbd>Cmd+O</kbd> or start adding
                objects to a new model.
              </p>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

function parseYaml(yamlStr: string): Model {
  const result = yaml.load(yamlStr, { schema: OPENBSE_SCHEMA });
  if (typeof result !== "object" || result === null) {
    return {};
  }
  return result as Model;
}

export default App;
