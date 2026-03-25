import { useEffect, useRef } from "react";

interface HelpDialogProps {
  open: boolean;
  onClose: () => void;
}

export function HelpDialog({ open, onClose }: HelpDialogProps) {
  const dialogRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const handleKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("keydown", handleKey);
    return () => document.removeEventListener("keydown", handleKey);
  }, [open, onClose]);

  if (!open) return null;

  return (
    <div className="help-overlay" onClick={onClose}>
      <div
        className="help-dialog"
        ref={dialogRef}
        onClick={(e) => e.stopPropagation()}
      >
        <div className="help-header">
          <h2>OpenBSE Workbench</h2>
          <span className="help-version">v0.1.0</span>
          <button className="btn-icon" onClick={onClose} title="Close">
            x
          </button>
        </div>
        <div className="help-body">
          <section className="help-section">
            <h3>Getting Started</h3>
            <p>
              OpenBSE Workbench is an integrated environment for editing building
              energy simulation models, viewing HVAC network topology, and
              exploring simulation results.
            </p>
            <ol>
              <li>
                <strong>Open Folder</strong> to load a project directory. The
                workbench auto-detects YAML model files and CSV result files.
              </li>
              <li>
                Or use <strong>Open YAML</strong> / <strong>Open CSV</strong> to
                load files individually.
              </li>
              <li>
                Switch between <strong>Edit</strong>, <strong>Network</strong>,
                and <strong>Charts</strong> views using the tabs in the header.
              </li>
            </ol>
          </section>

          <section className="help-section">
            <h3>Edit View</h3>
            <p>
              The model editor shows all object classes from the OpenBSE schema
              in the left panel. Click a class to edit its properties. Array
              classes (zones, surfaces, schedules, etc.) support multiple
              instances with add, duplicate, delete, and reorder controls.
            </p>
          </section>

          <section className="help-section">
            <h3>Network View</h3>
            <p>
              Visualizes the HVAC system topology from the loaded YAML model.
              Toggle between <strong>Air Side</strong> (air handlers, coils,
              fans, zones) and <strong>Water Side</strong> (pumps, boilers,
              chillers, cooling towers).
            </p>
            <p>
              Click any component node to see its output variables in the right
              panel. Check variables to select them for charting — selections
              persist when you switch to the Charts tab.
            </p>
          </section>

          <section className="help-section">
            <h3>Charts View</h3>
            <p>
              Time-series charts for simulation results. The left panel shows all
              CSV variables organized by zone or component type. Check variables
              to plot them. Features:
            </p>
            <ul>
              <li>
                <strong>Multiple Y-axes</strong> — variables are auto-grouped by
                unit
              </li>
              <li>
                <strong>Zoom &amp; pan</strong> — scroll wheel to zoom, drag the
                slider at the bottom, or use the box-zoom tool
              </li>
              <li>
                <strong>Aggregation</strong> — switch between Raw, Hourly,
                Daily, and Monthly averages
              </li>
              <li>
                <strong>Summary statistics</strong> — toggle the Stats panel to
                see min/max/mean/total for selected variables
              </li>
              <li>
                <strong>Variable browser modes</strong> — switch between "By
                Zone" and "By Component" grouping. Right-click to expand or
                collapse all.
              </li>
            </ul>
          </section>

          <section className="help-section">
            <h3>Running Simulations</h3>
            <p>
              In Edit view, the bottom bar has simulation controls. Select a
              weather file (EPW), then click <strong>Run Simulation</strong>. The
              model is auto-saved before running. Results are automatically
              loaded into the Charts view when the simulation completes.
            </p>
          </section>

          <section className="help-section">
            <h3>Keyboard Shortcuts</h3>
            <table className="help-shortcuts">
              <tbody>
                <tr>
                  <td><kbd>Cmd+N</kbd></td>
                  <td>New model</td>
                </tr>
                <tr>
                  <td><kbd>Cmd+O</kbd></td>
                  <td>Open YAML model</td>
                </tr>
                <tr>
                  <td><kbd>Cmd+S</kbd></td>
                  <td>Save model</td>
                </tr>
                <tr>
                  <td><kbd>Cmd+Shift+S</kbd></td>
                  <td>Save model as</td>
                </tr>
                <tr>
                  <td><kbd>Esc</kbd></td>
                  <td>Close this help dialog</td>
                </tr>
              </tbody>
            </table>
          </section>

          <section className="help-section help-footer-section">
            <p className="help-links">
              OpenBSE is open source.
              Visit <strong>github.com/bbrannon4/OpenBSE</strong> for
              documentation, issues, and source code.
            </p>
          </section>
        </div>
      </div>
    </div>
  );
}
