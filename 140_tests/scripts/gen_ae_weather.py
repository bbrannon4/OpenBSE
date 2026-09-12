#!/usr/bin/env python3
"""Generate constant-condition EPW weather files for ASHRAE 140 Section 11 (AE).

Each AE case runs at a single fixed outdoor condition (given as dry-bulb and
wet-bulb in the reference inputs). This writes a minimal constant-value EPW for
each unique (Tdb, Twb) pair, computing the humidity ratio / dew point / RH from
standard psychrometrics, into 140_tests/weather/.
"""

import math
import os

HERE = os.path.dirname(os.path.abspath(__file__))
WEATHER_DIR = os.path.join(os.path.dirname(HERE), "weather")
P_ATM = 101325.0  # Pa (sea level; AE elevation = 0)

# name -> (Tdb, Twb) in °C
CONDITIONS = {
    "AE_m29": (-29.0, -29.0),
    "AE_155": (15.5, 7.207),
    "AE_269": (26.9, 23.44),
    "AE_249": (24.9, 13.029),
    "AE_230": (23.0, 21.525),
}


def p_sat(t_c):
    """Saturation vapor pressure [Pa] (ASHRAE over water, t > 0; over ice below)."""
    t = t_c + 273.15
    if t_c >= 0:
        c = [-5800.2206, 1.3914993, -0.04860239, 4.1764768e-5, -1.4452093e-8, 6.5459673]
        return math.exp(c[0] / t + c[1] + c[2] * t + c[3] * t**2 + c[4] * t**3 + c[5] * math.log(t))
    c = [-5674.5359, 6.3925247, -0.009677843, 6.2215701e-7, 2.0747825e-9, -9.484024e-13, 4.1635019]
    return math.exp(c[0] / t + c[1] + c[2] * t + c[3] * t**2 + c[4] * t**3 + c[5] * t**4 + c[6] * math.log(t))


def w_from_tdb_twb(tdb, twb):
    """Humidity ratio [kg/kg] from dry-bulb and wet-bulb (ASHRAE 6.9/6.8)."""
    w_s_wb = 0.621945 * p_sat(twb) / (P_ATM - p_sat(twb))
    if tdb >= 0:
        return ((2501.0 - 2.326 * twb) * w_s_wb - 1.006 * (tdb - twb)) / (
            2501.0 + 1.86 * tdb - 4.186 * twb
        )
    # Below freezing (ASHRAE eq. over ice)
    return ((2830.0 - 0.24 * twb) * w_s_wb - 1.006 * (tdb - twb)) / (
        2830.0 + 1.86 * tdb - 2.1 * twb
    )


def dewpoint(w):
    pw = P_ATM * w / (0.621945 + w)
    if pw <= 0:
        return -60.0
    alpha = math.log(pw / 1000.0)
    # ASHRAE dew-point correlation (t >= 0 range; adequate here)
    td = 6.54 + 14.526 * alpha + 0.7389 * alpha**2 + 0.09486 * alpha**3 + 0.4569 * (pw / 1000.0) ** 0.1984
    return td


def rh(tdb, w):
    pw = P_ATM * w / (0.621945 + w)
    return max(1.0, min(100.0, 100.0 * pw / p_sat(tdb)))


HEADER = [
    "LOCATION,AE CONSTANT,,,Std140 AE,000000,25.8,-80.3,-5.0,0.0",
    "DESIGN CONDITIONS,0",
    "TYPICAL/EXTREME PERIODS,0",
    "GROUND TEMPERATURES,0",
    "HOLIDAYS/DAYLIGHT SAVINGS,No,0,0,0",
    "COMMENTS 1,ASHRAE 140 Section 11 constant-condition synthetic weather",
    "COMMENTS 2,",
    "DATA PERIODS,1,1,Data,Monday, 1/ 1,12/31",
]


def main():
    for name, (tdb, twb) in CONDITIONS.items():
        w = w_from_tdb_twb(tdb, twb)
        tdp = min(dewpoint(w), tdb)
        relh = rh(tdb, w)
        lines = list(HEADER)
        for day in range(1, 366):
            month, dom = _md(day)
            for hour in range(1, 25):
                # EPW fields: year,mon,day,hour,min,flags,Tdb,Tdp,RH,Patm,
                #   then ETR..DHI..cloud.. (zeros), wind dir/speed at 20/21.
                f = ["1999", str(month), str(dom), str(hour), "0",
                     "C9C9C9C9*0?9?9?9?9?9?9?9?9?9?9?9?9?9?9?9?9*9*9*9*9*9"]
                f += [f"{tdb:.1f}", f"{tdp:.1f}", f"{relh:.0f}", f"{P_ATM:.0f}"]
                f += ["0"] * 10          # ETR..DHI + illum placeholders (idx 10-19)
                f += ["0", "0.0"]        # wind dir (20), wind speed (21)
                f += ["0", "0"]          # idx 22,23 (total/opaque sky)
                lines.append(",".join(f))
        with open(os.path.join(WEATHER_DIR, f"{name}.epw"), "w") as fh:
            fh.write("\n".join(lines) + "\n")
        print(f"{name}: Tdb={tdb} Twb={twb} -> W={w:.5f} Tdp={tdp:.1f} RH={relh:.0f}%")


_CUM = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334, 365]


def _md(doy):
    for m in range(12):
        if doy <= _CUM[m + 1]:
            return m + 1, doy - _CUM[m]
    return 12, 31


if __name__ == "__main__":
    main()
