#!/usr/bin/env python3
"""Read OpenBSE output for the ASHRAE 140-2023 Section 9B (CE_b) cases.

One place that turns a case's hourly CSVs into the quantities Section 9B asks
for, shared by the acceptance gate (build_140_csv.py) and the portal submission
(generate_ashrae140_CE_b_submission.py) so the two can never disagree.

Definitions follow the Std140_CE_b output glossary:
  total electricity = compressor + outdoor (condenser) fan + indoor (supply) fan
  coil loads        = gross evaporator coil loads (they include supply-fan heat)
  COP2              = gross total coil load / (compressor + outdoor fan)
Hourly values are hourly-integrated (the mean of the four sub-hourly steps).
"""

import csv
import math
import os

HERE = os.path.dirname(os.path.abspath(__file__))
CASES_DIR = os.path.join(os.path.dirname(HERE), "cases")

# Result rows of the Section 9B annual table: (row label, case, months).
# CE500 is reported twice (full year and May-September); CE510 only for
# May-September. months=None means the whole year.
MAY_SEP = (5, 6, 7, 8, 9)
ANNUAL_ROWS = [
    ("CE300", "CE300", None), ("CE310", "CE310", None), ("CE320", "CE320", None),
    ("CE330", "CE330", None), ("CE340", "CE340", None), ("CE350", "CE350", None),
    ("CE360", "CE360", None), ("CE400", "CE400", None), ("CE410", "CE410", None),
    ("CE420", "CE420", None), ("CE430", "CE430", None), ("CE440", "CE440", None),
    ("CE500", "CE500", None), ("CE500 May-Sep", "CE500", MAY_SEP),
    ("CE510 May-Sep", "CE510", MAY_SEP), ("CE520", "CE520", None),
    ("CE522", "CE522", None), ("CE525", "CE525", None), ("CE530", "CE530", None),
    ("CE540", "CE540", None), ("CE545", "CE545", None),
]
CASES = sorted({case for _, case, _ in ANNUAL_ROWS}, key=lambda c: int(c[2:]))

# Hours with less compressor + outdoor-fan energy than this [Wh] are treated as
# "system off" when looking for hourly COP2 extremes.
COP2_MIN_ENERGY_WH = 1.0


def psat(t_c):
    """Saturation vapor pressure over water / ice [Pa] (ASHRAE Hyland-Wexler)."""
    tk = t_c + 273.15
    if t_c >= 0.0:
        return math.exp(-5.8002206e3 / tk + 1.3914993 - 4.8640239e-2 * tk
                        + 4.1764768e-5 * tk**2 - 1.4452093e-8 * tk**3
                        + 6.5459673 * math.log(tk))
    return math.exp(-5.6745359e3 / tk + 6.3925247 - 9.677843e-3 * tk
                    + 6.2215701e-7 * tk**2 + 2.0747825e-9 * tk**3
                    - 9.484024e-13 * tk**4 + 4.1635019 * math.log(tk))


def wet_bulb(t_c, w, p_pa):
    """Thermodynamic wet-bulb [C] by bisection on the ASHRAE psychrometer equation."""
    lo, hi = -40.0, t_c
    for _ in range(60):
        mid = 0.5 * (lo + hi)
        ps = psat(mid)
        ws = 0.62198 * ps / (p_pa - ps)
        w_calc = ((2501.0 - 2.326 * mid) * ws - 1.006 * (t_c - mid)) / (
            2501.0 + 1.86 * t_c - 4.186 * mid)
        if w_calc > w:
            hi = mid
        else:
            lo = mid
    return 0.5 * (lo + hi)


def _read(path):
    with open(path) as f:
        return list(csv.DictReader(f))


def _col(rows, needle):
    keys = [k for k in rows[0] if needle in k]
    if not keys:
        raise KeyError(f"no column matching {needle!r}")
    return [float(r[keys[0]]) for r in rows]


class CaseOutput:
    """Hourly series for one case (lists of 8760 values, hour 1-24 convention)."""

    def __init__(self, case):
        base = os.path.join(CASES_DIR, f"ashrae140_case{case}")
        hvac = _read(base + "_hvac_results.csv")
        zone = _read(base + "_zone_results.csv")
        site = _read(base + "_site_results.csv")
        self.case = case
        self.month = [int(r["Month"]) for r in hvac]
        self.day = [int(r["Day"]) for r in hvac]
        self.hour = [int(r["Hour"]) for r in hvac]
        self.n = len(hvac)
        # Electricity [Wh per hour]
        self.comp = _col(hvac, "compressor_power")
        self.odfan = _col(hvac, "condenser_fan_power")
        self.idfan = _col(hvac, "Supply Fan:electric_power")
        # Evaporator coil loads [Wh per hour]
        self.q_tot = _col(hvac, "Cooling Coil:total_load")
        self.q_sens = _col(hvac, "Cooling Coil:sensible_load")
        self.q_lat = _col(hvac, "Cooling Coil:latent_load")
        # Coil entering air (on-cycle mixed air)
        self.edb = _col(hvac, "Cooling Coil:inlet_temperature")
        edw = _col(hvac, "Cooling Coil:inlet_humidity_ratio")
        # Site
        self.odb = _col(site, "outdoor_temperature")
        self.pressure = _col(site, "barometric_pressure")
        self.out_w = [g / 1000.0 for g in _col(site, "humidity_ratio_gkg")]
        # Zone (humidity ratio is written in g/kg for resolution)
        self.idb = _col(zone, "temperature")
        self.rh = _col(zone, "relative_humidity")
        self.w = [g / 1000.0 for g in _col(zone, "humidity_ratio_gkg")]
        self.ewb = [wet_bulb(t, w, p) for t, w, p in zip(self.edb, edw, self.pressure)]
        self.elec = [c + o + i for c, o, i in zip(self.comp, self.odfan, self.idfan)]
        self.cop2 = [q / (c + o) if (c + o) > COP2_MIN_ENERGY_WH else None
                     for q, c, o in zip(self.q_tot, self.comp, self.odfan)]

    def index(self, months=None, from_month=None):
        """Hour indices in `months`, or from `from_month` to year end."""
        if months is not None:
            return [i for i in range(self.n) if self.month[i] in months]
        if from_month is not None:
            return [i for i in range(self.n) if self.month[i] >= from_month]
        return list(range(self.n))

    def day_index(self, month, day):
        return [i for i in range(self.n) if self.month[i] == month and self.day[i] == day]

    def totals(self, idx):
        """Annual-table quantities over the hours `idx` (energies in kWh)."""
        kwh = lambda s: sum(s[i] for i in idx) / 1000.0
        mean = lambda s: sum(s[i] for i in idx) / len(idx)
        comp, odfan, idfan = kwh(self.comp), kwh(self.odfan), kwh(self.idfan)
        q_tot = kwh(self.q_tot)
        return {
            "cool_total": comp + odfan + idfan,
            "compressor": comp,
            "cond_fan": odfan,
            "supply_fan": idfan,
            "coil_total": q_tot,
            "coil_sens": kwh(self.q_sens),
            "coil_lat": kwh(self.q_lat),
            "cop2": q_tot / (comp + odfan) if comp + odfan > 0 else 0.0,
            "idb": mean(self.idb),
            "humrat": mean(self.w),
            "rh": mean(self.rh),
        }

    def daily_average(self, month, day):
        """Average hourly values over one day (energies in Wh per hour)."""
        idx = self.day_index(month, day)
        mean = lambda s: sum(s[i] for i in idx) / len(idx)
        comp, odfan, idfan, q_tot = (mean(self.comp), mean(self.odfan),
                                     mean(self.idfan), mean(self.q_tot))
        return {
            "cool_total": comp + odfan + idfan, "compressor": comp,
            "cond_fan": odfan, "supply_fan": idfan, "coil_total": q_tot,
            "coil_sens": mean(self.q_sens), "coil_lat": mean(self.q_lat),
            "humrat": mean(self.w),
            "cop2": q_tot / (comp + odfan) if comp + odfan > 0 else 0.0,
            "odb": mean(self.odb), "edb": mean(self.edb),
        }

    def extreme(self, series, idx=None, largest=True):
        """(value, month, day, hour) of the largest / smallest hourly value,
        skipping hours where the series is None."""
        idx = self.index() if idx is None else idx
        cand = [i for i in idx if series[i] is not None]
        pick = max if largest else min
        i = pick(cand, key=lambda k: series[k])
        return series[i], self.month[i], self.day[i], self.hour[i]


def annual_results(row_case, months):
    """Annual-table metrics for one result row, or None if output is missing."""
    try:
        out = CaseOutput(row_case)
    except (FileNotFoundError, KeyError):
        return None
    return out.totals(out.index(months=months))
