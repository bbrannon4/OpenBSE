import { useState, useEffect, useRef, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

interface SimulationOutput {
  stream: "stdout" | "stderr";
  line: string;
}

interface SimulationDone {
  success: boolean;
  code: number | null;
  output_path: string | null;
}

interface SimulationPanelProps {
  modelPath: string | null;
  dirty: boolean;
  onSave: () => Promise<void>;
}

type SimStatus = "idle" | "running" | "success" | "error";

export function SimulationPanel({
  modelPath,
  dirty,
  onSave,
}: SimulationPanelProps) {
  const [weatherPath, setWeatherPath] = useState<string | null>(null);
  const [status, setStatus] = useState<SimStatus>("idle");
  const [outputLines, setOutputLines] = useState<SimulationOutput[]>([]);
  const [expanded, setExpanded] = useState(false);
  const [resultCsvPath, setResultCsvPath] = useState<string | null>(null);
  const outputRef = useRef<HTMLDivElement>(null);

  // Auto-scroll output
  useEffect(() => {
    if (outputRef.current) {
      outputRef.current.scrollTop = outputRef.current.scrollHeight;
    }
  }, [outputLines]);

  // Listen for simulation events
  useEffect(() => {
    const unlistenOutput = listen<SimulationOutput>(
      "simulation-output",
      (event) => {
        setOutputLines((prev) => [...prev, event.payload]);
      }
    );
    const unlistenDone = listen<SimulationDone>(
      "simulation-done",
      (event) => {
        setStatus(event.payload.success ? "success" : "error");
        setResultCsvPath(event.payload.output_path ?? null);
      }
    );

    return () => {
      unlistenOutput.then((f) => f());
      unlistenDone.then((f) => f());
    };
  }, []);

  const pickWeatherFile = useCallback(async () => {
    try {
      const selected = await openDialog({
        title: "Select Weather File",
        multiple: false,
        directory: false,
        filters: [
          { name: "EPW Weather", extensions: ["epw"] },
          { name: "All Files", extensions: ["*"] },
        ],
      });
      if (selected) {
        setWeatherPath(selected as string);
      }
    } catch (e) {
      console.error("Weather file picker error:", e);
    }
  }, []);

  const runSimulation = useCallback(async () => {
    if (!modelPath) return;

    // Auto-save if dirty
    if (dirty) {
      await onSave();
    }

    // Derive output path: same dir as model, with .csv extension
    const outputPath = modelPath.replace(/\.(yaml|yml)$/i, "_results.csv");

    setOutputLines([]);
    setResultCsvPath(null);
    setStatus("running");
    setExpanded(true);

    try {
      await invoke("run_simulation", {
        modelPath,
        weatherPath,
        outputPath,
      });
    } catch (e) {
      // invoke itself threw (e.g. binary not found, spawn failed).
      // The simulation-done event won't fire in this case.
      setStatus("error");
      setOutputLines((prev) => [
        ...prev,
        { stream: "stderr", line: String(e) },
      ]);
    }
  }, [modelPath, weatherPath, dirty, onSave]);

  const weatherFileName = weatherPath?.split("/").pop() ?? null;

  const statusLabel =
    status === "running"
      ? "Running..."
      : status === "success"
        ? "Complete"
        : status === "error"
          ? "Failed"
          : "";

  return (
    <div className="simulation-bar">
      <div className="simulation-toolbar">
        <div className="sim-toolbar-left">
          {statusLabel && (
            <span className={`sim-status sim-status-${status}`}>
              {statusLabel}
            </span>
          )}
          {outputLines.length > 0 && (
            <button
              className="btn-header"
              onClick={() => setExpanded(!expanded)}
              title={expanded ? "Collapse output" : "Expand output"}
            >
              {expanded ? "Hide Output" : "Show Output"}
            </button>
          )}
        </div>
        <div className="sim-toolbar-right">
          <button
            className="btn-header"
            onClick={pickWeatherFile}
            title="Select EPW weather file"
          >
            Weather
          </button>
          {weatherFileName && (
            <span className="sim-weather-name" title={weatherPath ?? undefined}>
              {weatherFileName}
            </span>
          )}
          <button
            className="btn-run"
            onClick={runSimulation}
            disabled={!modelPath || status === "running"}
            title="Run simulation (saves first if needed)"
          >
            {status === "running" ? "Running..." : "Run Simulation"}
          </button>
        </div>
      </div>
      {expanded && outputLines.length > 0 && (
        <div className="simulation-output" ref={outputRef}>
          {outputLines.map((line, i) => (
            <div
              key={i}
              className={`sim-line ${line.stream === "stderr" ? "sim-line-err" : ""}`}
            >
              {line.line}
            </div>
          ))}
          {resultCsvPath && status === "success" && (
            <div className="sim-line sim-result">
              Results saved to: {resultCsvPath}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
