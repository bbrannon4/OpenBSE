#!/usr/bin/env python3
"""Convert an ASHRAE-140 TMY2 (.TM2) weather file to the TMY3-CSV format
OpenBSE reads (same layout as the CE_a CExxxA.csv files)."""
import sys, os

def g(line, a, b):  # 0-indexed [a:b], int, '' -> 0
    s = line[a:b].strip()
    return int(s) if s and s.lstrip("-").isdigit() else 0

def convert(tm2_path, csv_path):
    lines = open(tm2_path).read().splitlines()
    hdr = lines[0]
    wmo = hdr[1:6].strip()
    city = hdr[7:29].strip()
    state = hdr[30:32].strip()
    tz = hdr[33:36].strip()
    # lat: N dd mm ; lon: W ddd mm  (header is fixed-ish; parse tokens)
    toks = hdr[36:].split()
    # toks: ['N','29','59','W','90','15','3']
    latsign = 1 if toks[0]=='N' else -1
    lat = latsign*(int(toks[1]) + int(toks[2])/60.0)
    lonsign = -1 if toks[3]=='W' else 1
    lon = lonsign*(int(toks[4]) + int(toks[5])/60.0)
    elev = int(toks[6])
    out = []
    out.append(f'{wmo},"{city}",{state},{tz},{lat:.2f},{lon:.2f},{elev}')
    cols = ('Date (MM/DD/YYYY),Time (HH:MM),ETR (W/m^2),ETRN (W/m^2),'
            'GHI (W/m^2),GHI source,GHI uncert (%),DNI (W/m^2),DNI source,DNI uncert (%),'
            'DHI (W/m^2),DHI source,DHI uncert (%),GH illum (lx),GH illum source,Global illum uncert (%),'
            'DN illum (lx),DN illum source,DN illum uncert (%),DH illum (lx),DH illum source,DH illum uncert (%),'
            'Zenith lum (cd/m^2),Zenith lum source,Zenith lum uncert (%),TotCld (tenths),TotCld source,TotCld uncert (code),'
            'OpqCld (tenths),OpqCld source,OpqCld uncert (code),Dry-bulb (C),Dry-bulb source,Dry-bulb uncert (code),'
            'Dew-point (C),Dew-point source,Dew-point uncert (code),RHum (%),RHum source,RHum uncert (code),'
            'Pressure (mbar),Pressure source,Pressure uncert (code),Wdir (degrees),Wdir source,Wdir uncert (code),'
            'Wspd (m/s),Wspd source,Wspd uncert (code),Hvis (m),Hvis source,Hvis uncert (code),'
            'CeilHgt (m),CeilHgt source,CeilHgt uncert (code),Pwat (cm),Pwat source,Pwat uncert (code),'
            'AOD (unitless),AOD source,AOD uncert (code),Alb (unitless),Alb source,Alb uncert (code),'
            'Lprecip depth (mm),Lprecip quantity (hr),Lprecip source,Lprecip uncert (code)')
    out.append(cols)
    for ln in lines[1:]:
        if len(ln) < 98: continue
        yr=g(ln,1,3); mo=g(ln,3,5); da=g(ln,5,7); hr=g(ln,7,9)
        etr=g(ln,9,13); etrn=g(ln,13,17)
        ghi=g(ln,17,21); dni=g(ln,23,27); dhi=g(ln,29,33)
        tsc=g(ln,59,61); osc=g(ln,63,65)
        tdb=g(ln,67,71)/10.0; tdew=g(ln,73,77)/10.0; rh=g(ln,79,82)
        pres=g(ln,84,88); wdir=g(ln,90,93); wspd=g(ln,95,98)/10.0
        year=1900+yr if yr>=50 else 2000+yr
        row=(f'{mo:02d}/{da:02d}/{year},{hr:02d}:00,{etr},{etrn},'
             f'{ghi},1,0,{dni},1,0,{dhi},1,0,0,1,0,0,1,0,0,1,0,0,1,0,'
             f'{tsc},A,7,{osc},A,7,{tdb:.1f},A,7,{tdew:.1f},A,7,{rh},A,7,'
             f'{pres},A,7,{wdir},A,7,{wspd:.1f},A,7,-9900,?,0,-9900,?,0,'
             f'-9900,?,0,-9900,?,0,-9900,?,0,-9900,-9900,?,0')
        out.append(row)
    open(csv_path,'w').write("\n".join(out)+"\n")
    return out

tm2="/Users/benjaminbrannon/Downloads/Accompanying FIles/Std140_CE_b_Files/Normative Materials/CE300.TM2"
csv="/Users/benjaminbrannon/Documents/GitHub/OpenBSE/140_tests/weather/CE300.csv"
rows=convert(tm2,csv)
print("header:", rows[0])
print("rows written:", len(rows)-2)
print("\n=== sanity: first 6 hours (date, time, dry-bulb, dew-point, RH, wind) ===")
for r in rows[2:8]:
    f=r.split(","); print(f" {f[0]} {f[1]}  Tdb={f[31]}  Tdew={f[34]}  RH={f[37]}  Wspd={f[46]}")
# annual min/max/mean dry-bulb as a sanity check for New Orleans
import statistics
tdbs=[float(r.split(",")[31]) for r in rows[2:]]
print(f"\nAnnual dry-bulb: min={min(tdbs):.1f} max={max(tdbs):.1f} mean={statistics.mean(tdbs):.1f} (New Orleans expect ~ -5..37, mean ~20)")
