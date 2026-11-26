#!/usr/bin/env python3
"""Convert an ASHRAE-140 WYEC2 (.WY2) weather file to the TMY3-CSV format
OpenBSE reads, field for field, as convert_tm2_weather.py does for TMY2.

Section 10 (HE100-HE230) names the HE100W, HE120W, HE130W, HE140W and HE210W
WYEC2 files as its normative weather. Fixed-column layout per the Furnace
BESTEST report, Appendix (WYEC2 format): irradiance at columns 19-22
(global), 25-28 (direct normal) and 31-34 (diffuse) in kJ/m2 per hour,
pressure in units of 10 Pa, temperatures in 0.1 C, wind speed in 0.1 m/s,
sky cover in tenths.

WYEC2 carries dew point but no relative humidity; the RH column is computed
from dry-bulb and dew point with the ASHRAE (Hyland-Wexler) saturation
pressure, over water above 0 C and over ice below, so it agrees with the
file's dew point.

usage: convert_wy2_weather.py <normative .WY2> <output .csv> [location header]
"""
import math
import sys

KJ_TO_WH = 1.0 / 3.6  # kJ/m2 in one hour -> W/m2


def p_sat(t_c):
    """Saturation vapour pressure [Pa], ASHRAE Fundamentals (Hyland-Wexler)."""
    t = t_c + 273.15
    if t_c >= 0.0:
        c = (-5800.2206, 1.3914993, -0.048640239, 4.1764768e-5, -1.4452093e-8, 6.5459673)
        return math.exp(c[0] / t + c[1] + c[2] * t + c[3] * t**2 + c[4] * t**3 + c[5] * math.log(t))
    c = (-5674.5359, 6.3925247, -0.009677843, 6.2215701e-7, 2.0747825e-9, -9.484024e-13, 4.1635019)
    return math.exp(
        c[0] / t + c[1] + c[2] * t + c[3] * t**2 + c[4] * t**3 + c[5] * t**4 + c[6] * math.log(t)
    )


DAYS_IN_MONTH = (31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31)


def month_day_hour(index):
    """(month, day, hour 1-24) of the index-th hour of a non-leap year."""
    day_of_year, hour = divmod(index, 24)
    month = 0
    while day_of_year >= DAYS_IN_MONTH[month]:
        day_of_year -= DAYS_IN_MONTH[month]
        month += 1
    return month + 1, day_of_year + 1, hour + 1


def field(line, a, b):
    s = line[a:b].strip()
    return int(s) if s and s.lstrip("-").isdigit() else 0


def convert(wy2_path, csv_path, header):
    rows = []
    records = [ln for ln in open(wy2_path).read().splitlines() if len(ln) >= 113]
    for i, ln in enumerate(records):
        # WYEC2 records are positional: record i is hour i of the year. The
        # date stamps are not trusted, because the Section 10 files label
        # their 40th record (Jan 2, hour 16) as hour 15 a second time.
        yr = field(ln, 6, 8)
        mo, da, hr = month_day_hour(i)
        etr = field(ln, 14, 18) * KJ_TO_WH
        ghi = field(ln, 18, 22) * KJ_TO_WH
        dni = field(ln, 24, 28) * KJ_TO_WH
        dhi = field(ln, 30, 34) * KJ_TO_WH
        pres_mbar = field(ln, 83, 88) / 10.0  # units of 10 Pa -> mbar
        tdb = field(ln, 89, 93) / 10.0
        tdp = field(ln, 94, 98) / 10.0
        wdir = field(ln, 99, 102)
        wspd = field(ln, 102, 107) / 10.0
        tsc, osc = field(ln, 107, 110), field(ln, 110, 113)
        rh = min(100.0, 100.0 * p_sat(tdp) / p_sat(tdb))
        year = 1900 + yr if yr >= 50 else 2000 + yr
        rows.append(
            f"{mo:02d}/{da:02d}/{year},{hr:02d}:00,{etr:.0f},0,"
            f"{ghi:.1f},1,0,{dni:.1f},1,0,{dhi:.1f},1,0,0,1,0,0,1,0,0,1,0,0,1,0,"
            f"{tsc},A,7,{osc},A,7,{tdb:.1f},A,7,{tdp:.1f},A,7,{rh:.2f},A,7,"
            f"{pres_mbar:.1f},A,7,{wdir},A,7,{wspd:.1f},A,7,-9900,?,0,-9900,?,0,"
            f"-9900,?,0,-9900,?,0,-9900,?,0,-9900,-9900,?,0"
        )
    if len(rows) != 8760:
        sys.exit(f"{wy2_path}: expected a year of hourly records, got {len(rows)}")
    cols = (
        "Date (MM/DD/YYYY),Time (HH:MM),ETR (W/m^2),ETRN (W/m^2),GHI (W/m^2),GHI source,"
        "GHI uncert (%),DNI (W/m^2),DNI source,DNI uncert (%),DHI (W/m^2),DHI source,"
        "DHI uncert (%),GH illum (lx),GH illum source,Global illum uncert (%),DN illum (lx),"
        "DN illum source,DN illum uncert (%),DH illum (lx),DH illum source,DH illum uncert (%),"
        "Zenith lum (cd/m^2),Zenith lum source,Zenith lum uncert (%),TotCld (tenths),"
        "TotCld source,TotCld uncert (code),OpqCld (tenths),OpqCld source,OpqCld uncert (code),"
        "Dry-bulb (C),Dry-bulb source,Dry-bulb uncert (code),Dew-point (C),Dew-point source,"
        "Dew-point uncert (code),RHum (%),RHum source,RHum uncert (code),Pressure (mbar),"
        "Pressure source,Pressure uncert (code),Wdir (degrees),Wdir source,Wdir uncert (code),"
        "Wspd (m/s),Wspd source,Wspd uncert (code),Hvis (m),Hvis source,Hvis uncert (code),"
        "CeilHgt (m),CeilHgt source,CeilHgt uncert (code),Pwat (cm),Pwat source,"
        "Pwat uncert (code),AOD (unitless),AOD source,AOD uncert (code),Alb (unitless),"
        "Alb source,Alb uncert (code),Lprecip depth (mm),Lprecip quantity (hr),"
        "Lprecip source,Lprecip uncert (code)"
    )
    with open(csv_path, "w") as f:
        f.write(header + "\n" + cols + "\n" + "\n".join(rows) + "\n")
    return len(rows)


if __name__ == "__main__":
    # The WYEC2 records carry no location header. Station 04772 is Ottawa
    # International (45.32 N, 75.67 W, UTC-5, 114 m); the Section 10 cases use
    # no solar or long-wave exchange, so only the time zone matters.
    if len(sys.argv) not in (3, 4):
        sys.exit(__doc__)
    hdr = sys.argv[3] if len(sys.argv) == 4 else '716280,"OTTAWA INTL (WYEC2 04772)",ON,-5.0,45.32,-75.67,114'
    n = convert(sys.argv[1], sys.argv[2], hdr)
    print(f"rows written: {n}")
