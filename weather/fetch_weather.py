#!/usr/bin/env python3
"""Download the weather files the shipped models need but the repo can't ship.

The TMY3 files in this directory are NREL data (public domain) and are
committed. Climate.OneBuilding TMYx files are not: the site marks its content
"All Rights Reserved", so they are fetched here instead of redistributed.

OneBuilding re-issues files in place (e.g. the 2025 re-issue only rewrote the
DESIGN CONDITIONS header line). The engine ignores that line, so the pin is
the SHA-256 of the hourly data: everything after the 8 header lines, with
line endings normalised. A data change fails the pin; a header refresh doesn't.

Usage: python3 weather/fetch_weather.py      (skips files already present)
"""

import hashlib
import io
import sys
import urllib.request
import zipfile
from pathlib import Path

HERE = Path(__file__).resolve().parent

ONEBUILDING = "https://climate.onebuilding.org/WMO_Region_4_North_and_Central_America/USA_United_States_of_America"

FILES = {
    "USA_CO_Boulder.Muni.AP.720533_TMYx.2009-2023.epw": (
        f"{ONEBUILDING}/CO_Colorado/USA_CO_Boulder.Muni.AP.720533_TMYx.2009-2023.zip",
        "e6195c4fcd9cf28c9ce3e4dc96c996fea1f79de26123b4eb4339a2ed1bd8bd8e",
    ),
}


def data_hash(epw_bytes: bytes) -> str:
    lines = epw_bytes.replace(b"\r", b"").split(b"\n")
    return hashlib.sha256(b"\n".join(lines[8:])).hexdigest()


def fetch(name: str, url: str, want: str) -> None:
    req = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0 (OpenBSE CI)"})
    with urllib.request.urlopen(req, timeout=120) as resp:
        archive = zipfile.ZipFile(io.BytesIO(resp.read()))
    epw = archive.read(name)
    got = data_hash(epw)
    if got != want:
        sys.exit(f"{name}: hourly data hash {got} != pinned {want}; the source data changed")
    (HERE / name).write_bytes(epw)
    print(f"fetched {name}")


def main() -> None:
    for name, (url, want) in FILES.items():
        path = HERE / name
        if path.exists():
            if data_hash(path.read_bytes()) != want:
                sys.exit(f"{path}: hourly data doesn't match the pinned hash; delete it and re-run")
            print(f"present {name}")
            continue
        fetch(name, url, want)


if __name__ == "__main__":
    main()
