"""Program identity for the ASHRAE 140 output forms.

The version comes from the workspace Cargo.toml and its release date from the
matching CHANGELOG heading, so the forms always name the build that produced
them (Normative Annex A2 asks for the program version and its release date).
"""

import datetime
import os
import re

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def version():
    """Workspace version, e.g. "0.8.0"."""
    with open(os.path.join(ROOT, "Cargo.toml")) as f:
        text = f.read()
    m = re.search(r'\[workspace\.package\][^\[]*?^version\s*=\s*"([^"]+)"', text, re.M | re.S)
    if not m:
        raise SystemExit("no [workspace.package] version in Cargo.toml")
    return m.group(1)


def release_date():
    """Date of this version's CHANGELOG heading, or today if it isn't there yet."""
    with open(os.path.join(ROOT, "CHANGELOG.md")) as f:
        m = re.search(rf"^## \[{re.escape(version())}\] - (\d{{4}}-\d{{2}}-\d{{2}})", f.read(), re.M)
    return datetime.date.fromisoformat(m.group(1)) if m else datetime.date.today()


def submission_date():
    return datetime.date.today()
