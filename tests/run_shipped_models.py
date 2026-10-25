#!/usr/bin/env python3
"""Run every shipped model, apply sanity gates, and compare against baselines.

Every YAML under examples/ and prototype_tests/ must have an entry in
tests/model_checks.json (its tier, and any documented exception). For each
model in the selected tier this script:

  1. runs the release binary (which parses and validates the model; a severe
     validation error exits non-zero);
  2. requires a zero exit code;
  3. applies sanity gates to the outputs: zone temperatures within limits, no
     NaN/inf in any column, no negative capacity / part-load ratio, no COP <= 0
     while running, no negative sized capacity or airflow, and annual unmet
     hours under the model's threshold;
  4. compares annual end uses and unmet hours against tests/model_baselines.json
     (fails on a change larger than the tolerance in model_checks.json).

See tests/README.md for running locally and updating baselines.
"""

import argparse
import csv
import hashlib
import json
import os
import platform
import re
import shutil
import subprocess
import sys
import tempfile
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CONFIG = ROOT / "tests" / "model_checks.json"
BASELINES = ROOT / "tests" / "model_baselines.json"
MODEL_DIRS = ("examples", "prototype_tests")

csv.field_size_limit(sys.maxsize)


# ── Discovery and config ─────────────────────────────────────────────────────


def discover_models() -> list[str]:
    found = []
    for d in MODEL_DIRS:
        found += [p.relative_to(ROOT).as_posix() for p in (ROOT / d).rglob("*.yaml")]
    return sorted(found)


def load_config() -> dict:
    cfg = json.loads(CONFIG.read_text())
    models = discover_models()
    problems = [f"{m}: shipped model has no entry in {CONFIG.name}" for m in models if m not in cfg["models"]]
    problems += [f"{m}: in {CONFIG.name} but no such model" for m in cfg["models"] if m not in models]
    for name, entry in cfg["models"].items():
        if entry.get("tier") not in ("fast", "slow", "skip"):
            problems.append(f"{name}: tier must be fast, slow or skip")
        if entry.get("tier") == "skip" and not entry.get("reason"):
            problems.append(f"{name}: a skipped model needs a reason")
        for key in ("max_unmet_heating_hours", "max_unmet_cooling_hours", "zone_temp_limits_c", "weather"):
            if key in entry and not entry.get(f"{key}_reason"):
                problems.append(f"{name}: {key} overrides the default, so it needs {key}_reason")
    if problems:
        sys.exit("model_checks.json is out of date:\n  " + "\n  ".join(problems))
    return cfg


# ── Memory guard (the owner's laptop crashed from parallel prototype runs) ───


def memory_pressure_ok() -> bool:
    """False while macOS reports warn/critical memory pressure. Other OSes: True."""
    if platform.system() != "Darwin":
        return True
    try:
        out = subprocess.run(
            ["sysctl", "-n", "kern.memorystatus_vm_pressure_level"], capture_output=True, text=True, timeout=5
        ).stdout.strip()
        return out in ("", "1")
    except (OSError, subprocess.SubprocessError):
        return True


def wait_for_memory(name: str) -> None:
    warned = False
    while not memory_pressure_ok():
        if not warned:
            print(f"  waiting for memory pressure to return to normal before {name}", flush=True)
            warned = True
        time.sleep(10)


# ── Output parsing and gates ─────────────────────────────────────────────────

NUM = re.compile(r"^-?\d")
PLR_VARS = {"plr", "part_load_ratio", "rtf"}


def var_name(col: str) -> tuple[str, str]:
    """'RTU DX Cooling:cop_operating [-]' -> ('RTU DX Cooling', 'cop_operating')."""
    col = col.split(" [", 1)[0]
    comp, _, var = col.rpartition(":")
    return comp, var


def scan_results(path: Path, limits: list, zone_limits: dict) -> tuple[list[str], dict]:
    """Stream one timestep results CSV. Returns (failures, annual values).

    zone_limits overrides the [lo, hi] zone temperature limits for named zones."""
    failures: list[str] = []
    with path.open(newline="") as f:
        header = next(csv.reader([f.readline()]))
        cols = [var_name(c) for c in header]
        temp_idx = [i for i, (_, v) in enumerate(cols) if v == "zone_temp"]
        neg_idx = [i for i, (_, v) in enumerate(cols) if v in PLR_VARS or "capacity" in v]
        cop_idx = [i for i, (_, v) in enumerate(cols) if v.startswith("cop")]
        plr_of = {c: i for i, (c, v) in enumerate(cols) if v == "plr"}
        cop_pairs = [(i, plr_of.get(cols[i][0])) for i in cop_idx]
        elec_idx = [i for i, (_, v) in enumerate(cols) if v == "electric_power"]
        fuel_idx = [i for i, (_, v) in enumerate(cols) if v == "fuel_power"]
        subhour = header.index("SubHour") if "SubHour" in header else None

        stats: dict[int, list] = {}  # col -> [min, max]
        bad_cop: dict[int, tuple] = {}
        nonfinite: dict[int, int] = {}
        elec = fuel = temp_sum = 0.0
        rows = max_sub = 0
        # Data rows are plain numbers (no quoting), so a split is enough and
        # far faster than csv on multi-GB prototype outputs. Rust writes
        # non-finite values as NaN / inf, the only data cells with N, n or i.
        for line in f:
            rows += 1
            row = line.rstrip("\r\n").split(",")
            if "N" in line or "n" in line or "i" in line:
                for i, v in enumerate(row):
                    if v.lower().lstrip("-+") in ("nan", "inf", "infinity"):
                        nonfinite.setdefault(i, rows)
            for i in temp_idx:
                t = float(row[i])
                temp_sum += t
                s = stats.setdefault(i, [t, t])
                s[0] = min(s[0], t)
                s[1] = max(s[1], t)
            for i in neg_idx:
                x = float(row[i])
                if x < -1e-9:
                    s = stats.setdefault(i, [x, x])
                    s[0] = min(s[0], x)
            for i, p in cop_pairs:
                cop = float(row[i])
                running = p is not None and float(row[p]) > 1e-6
                if (cop < 0 or (running and cop <= 0)) and i not in bad_cop:
                    bad_cop[i] = (cop, rows)
            elec += sum(float(row[i]) for i in elec_idx)
            fuel += sum(float(row[i]) for i in fuel_idx)
            if subhour is not None:
                max_sub = max(max_sub, int(row[subhour]))

    for i, r in nonfinite.items():
        failures.append(f"{path.name}: {header[i]} is NaN/inf (first at data row {r})")
    for i in temp_idx:
        lo, hi = zone_limits.get(cols[i][0], limits)
        if i in stats and (stats[i][0] < lo or stats[i][1] > hi):
            failures.append(f"{path.name}: {header[i]} spans {stats[i][0]:.1f}..{stats[i][1]:.1f} °C, outside [{lo}, {hi}]")
    for i in neg_idx:
        if i in stats:
            failures.append(f"{path.name}: {header[i]} goes negative (min {stats[i][0]:.4g})")
    for i, (cop, r) in bad_cop.items():
        failures.append(f"{path.name}: {header[i]} = {cop:.4g} while running (data row {r})")
    hours_per_step = 1.0 / max(max_sub, 1)
    values = {
        "Results CSV electric [kWh]": round(elec * hours_per_step / 1000.0, 1),
        "Results CSV fuel [kWh]": round(fuel * hours_per_step / 1000.0, 1),
    }
    if temp_idx and rows:
        # Guards envelope-only models, which have no end uses to compare.
        values["Mean zone temp [C]"] = round(temp_sum / (rows * len(temp_idx)), 3)
    return failures, values


SIZING_LABEL = re.compile(r"capacity|airflow|flow rate|peak", re.I)


def scan_sizing(path: Path) -> list[str]:
    """No sized capacity or airflow may be negative (e.g. an autosize sentinel)."""
    failures = []
    header: list[str] = []
    with path.open(newline="") as f:
        for row in csv.reader(f):
            if not row or not any(row):
                header = []
                continue
            if not any(NUM.match(c.strip()) for c in row[1:]):
                header = row
                continue
            for j, cell in enumerate(row[1:], start=1):
                label = f"{row[0]} {header[j] if j < len(header) else ''}"
                try:
                    v = float(cell)
                except ValueError:
                    continue
                if v < 0 and SIZING_LABEL.search(label):
                    col = header[j] if j < len(header) else ""
                    failures.append(f"{path.name}: {row[0]} {col} = {v}".replace("  ", " "))
    return failures


def read_summary(path: Path) -> dict:
    """Annual end uses (Total column) and unmet hours from <stem>_summary.csv."""
    values = {}
    section = None
    with path.open(newline="") as f:
        for row in csv.reader(f):
            if len(row) == 1:
                section = row[0]
                continue
            if section == "Monthly Energy End-Use [kWh]" and len(row) == 14 and row[0] != "End Use":
                values[row[0]] = float(row[13])
            elif section == "Unmet Hours" and len(row) == 2:
                values[row[0]] = float(row[1])
    return values


# ── Running one model ────────────────────────────────────────────────────────


def model_id(model: str) -> str:
    return Path(model).with_suffix("").as_posix().replace("/", "__")


def run_model(model: str, entry: dict, cfg: dict, binary: Path, out_root: Path, keep: bool) -> dict:
    stem = Path(model).stem
    out_dir = out_root / model_id(model)
    shutil.rmtree(out_dir, ignore_errors=True)
    out_dir.mkdir(parents=True)
    cmd = [str(binary), model, "-o", str(out_dir / f"{stem}_results.csv")]
    if "weather" in entry:
        cmd += ["-w", entry["weather"]]

    wait_for_memory(model)
    t0 = time.time()
    with (out_dir / "run.log").open("w") as log:
        try:
            proc = subprocess.run(cmd, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT, timeout=cfg["timeout_s"])
            code = proc.returncode
        except subprocess.TimeoutExpired:
            code = "timeout"
    result = {"model": model, "seconds": round(time.time() - t0, 1), "failures": [], "values": {}}

    if code != 0:
        tail = (out_dir / "run.log").read_text(errors="replace").splitlines()[-15:]
        result["failures"].append(f"exit code {code}; last log lines:\n      " + "\n      ".join(tail))
    else:
        results_csvs = sorted(out_dir.glob(f"{stem}*_results.csv"))
        results_csvs = [p for p in results_csvs if not p.name.endswith("_zone_results.csv")]
        parametric = len(results_csvs) > 1
        for p in results_csvs:
            fails, values = scan_results(p, cfg["zone_temp_limits_c"], entry.get("zone_temp_limits_c", {}))
            result["failures"] += fails
            if not parametric:
                result["values"].update((k, v) for k, v in values.items() if k.startswith("Mean"))
            elif p.name != f"{stem}_results.csv":
                run = p.name[len(stem) + 1 : -len("_results.csv")]
                result["values"].update((f"{run}: {k}", v) for k, v in values.items())
        for p in sorted(out_dir.glob("*_sizing.csv")):
            result["failures"] += scan_sizing(p)
        summary = out_dir / f"{stem}_summary.csv"
        if summary.exists():
            result["values"].update(read_summary(summary))
            for kind in ("heating", "cooling"):
                limit = entry.get(f"max_unmet_{kind}_hours", cfg["max_unmet_hours"])
                got = result["values"].get(f"Unmet {kind.capitalize()} Hours", 0.0)
                if got > limit:
                    result["failures"].append(f"{got:.0f} unmet {kind} hours > limit {limit}")
        elif not parametric:
            result["failures"].append("no summary CSV written")
        if not results_csvs:
            result["failures"].append("no results CSV written")

    if not keep:
        shutil.rmtree(out_dir, ignore_errors=True)
    return result


# ── Baselines ────────────────────────────────────────────────────────────────


def compare(values: dict, base: dict | None, cfg: dict) -> list[str]:
    if base is None:
        return ["no baseline (run with --update-baselines and commit the result)"]
    rel = cfg["baseline_rel_tol"]
    floor = cfg["baseline_abs_tol"]
    out = []
    for key in sorted(set(values) | set(base)):
        if key not in values or key not in base:
            out.append(f"{key}: {'new' if key not in base else 'missing'} (baseline {base.get(key)}, now {values.get(key)})")
            continue
        a, b = values[key], base[key]
        unit = "hours" if "Hours" in key else "C" if "[C]" in key else "kWh"
        tol = max(rel * abs(b), floor[unit])
        if abs(a - b) > tol:
            pct = f"{100 * (a - b) / b:+.2f}%" if b else "n/a"
            out.append(f"{key}: {b} -> {a} ({pct})")
    return out


def write_baselines(results: dict) -> None:
    """Store the values of every model that ran and passed its gates."""
    baselines = json.loads(BASELINES.read_text()) if BASELINES.exists() else {}
    for m, r in results.items():
        if not r["failures"] and r["values"]:
            baselines[m] = dict(sorted(r["values"].items()))
    BASELINES.write_text(json.dumps(dict(sorted(baselines.items())), indent=2) + "\n")
    digest = hashlib.sha256(BASELINES.read_bytes()).hexdigest()
    print(f"\nwrote {BASELINES.relative_to(ROOT)}; update its pinned hash in .github/workflows/ci.yml:")
    print(f"  {digest}  tests/model_baselines.json")


def main() -> None:
    sys.stdout.reconfigure(line_buffering=True)
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--tier", choices=["fast", "slow", "all"], default="fast")
    ap.add_argument("--models", nargs="*", help="only these models (paths as in model_checks.json)")
    ap.add_argument("--binary", type=Path, help="openbse binary (default: $CARGO_TARGET_DIR or target/, release)")
    ap.add_argument("--jobs", type=int, default=2, help="parallel runs for non-heavy models (heavy ones always run alone)")
    ap.add_argument("--out", type=Path, help="scratch output dir (default: a temp dir; keep it outside ~/Documents)")
    ap.add_argument("--keep-outputs", action="store_true")
    ap.add_argument("--update-baselines", action="store_true", help="write measured values into model_baselines.json")
    ap.add_argument("--report", type=Path, help="write measured values and failures as JSON (CI uploads this)")
    ap.add_argument("--baselines-from", type=Path, nargs="+", metavar="REPORT",
                    help="update baselines from --report files (e.g. CI's artifact) instead of running")
    args = ap.parse_args()

    cfg = load_config()
    if args.baselines_from:
        results = {}
        for report in args.baselines_from:
            results.update(json.loads(report.read_text()))
        write_baselines(results)
        return
    binary = args.binary or Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")) / "release" / "openbse"
    if not binary.exists():
        sys.exit(f"binary not found: {binary} (cargo build --release first)")
    out_root = args.out or Path(tempfile.mkdtemp(prefix="openbse-model-checks-"))
    out_root.mkdir(parents=True, exist_ok=True)

    selected, skipped = [], []
    for model, entry in cfg["models"].items():
        if args.models and model not in args.models:
            continue
        if entry["tier"] == "skip":
            skipped.append((model, entry["reason"]))
        elif args.models or args.tier == "all" or entry["tier"] == args.tier:
            selected.append(model)
    if args.models and (unknown := set(args.models) - set(cfg["models"])):
        sys.exit(f"unknown models: {sorted(unknown)}")

    print(f"binary: {binary}\noutputs: {out_root}\nrunning {len(selected)} model(s)")
    for model, reason in skipped:
        print(f"  SKIP {model}: {reason}")

    heavy = [m for m in selected if cfg["models"][m].get("heavy")]
    light = [m for m in selected if m not in heavy]
    results = {}

    def one(m: str) -> None:
        r = run_model(m, cfg["models"][m], cfg, binary, out_root, args.keep_outputs)
        results[m] = r
        print(f"  {'ok  ' if not r['failures'] else 'FAIL'} {m} ({r['seconds']} s)", flush=True)

    with ThreadPoolExecutor(max_workers=max(1, args.jobs)) as pool:
        list(pool.map(one, light))
    for m in heavy:
        one(m)

    baselines = json.loads(BASELINES.read_text()) if BASELINES.exists() else {}
    for m in selected:
        results[m]["baseline_diffs"] = compare(results[m]["values"], baselines.get(m), cfg)

    if args.update_baselines:
        write_baselines(results)

    if args.report:
        args.report.write_text(json.dumps(results, indent=2) + "\n")

    lines = ["| Model | Time [s] | Result |", "|---|---:|---|"]
    failed = 0
    for m in selected:
        r = results[m]
        problems = r["failures"] + ([] if args.update_baselines else r.get("baseline_diffs", []))
        failed += bool(problems)
        lines.append(f"| `{m}` | {r['seconds']} | {'ok' if not problems else '**FAIL**: ' + '; '.join(p.splitlines()[0] for p in problems)} |")
    for model, reason in skipped:
        lines.append(f"| `{model}` | – | skipped: {reason} |")
    table = "\n".join(lines)
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as f:
            f.write(f"### Shipped models ({args.tier})\n\n{table}\n")

    print()
    for m in selected:
        r = results[m]
        problems = r["failures"] + ([] if args.update_baselines else r.get("baseline_diffs", []))
        if problems:
            print(f"FAIL {m}")
            for p in problems:
                print(f"    {p}")
    print(f"\n{len(selected) - failed}/{len(selected)} passed, {len(skipped)} skipped")
    if not args.out and not args.keep_outputs:
        shutil.rmtree(out_root, ignore_errors=True)
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
