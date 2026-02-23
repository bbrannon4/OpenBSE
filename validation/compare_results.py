#!/usr/bin/env python3
"""Compare OpenBSE vs EnergyPlus zone temperatures for 1ZoneUncontrolled."""

import csv
import sys

# ── Parse EnergyPlus ESO file ──────────────────────────────────────────────

def parse_eso(eso_path):
    """Parse E+ ESO file, extracting timestep-level zone mean air temp and outdoor temp."""

    # Variable IDs from the header:
    # 7 = Site Outdoor Air Drybulb Temperature [C] !TimeStep
    # 207 = ZONE ONE Zone Mean Air Temperature [C] !TimeStep
    # 63 = Wall001 Inside Face Temperature [C] !TimeStep
    # 183 = Roof001 Inside Face Temperature [C] !TimeStep
    # 159 = Floor001 Inside Face Temperature [C] !TimeStep

    VAR_OUTDOOR = 7
    VAR_ZONE_TEMP = 207
    VAR_WALL001_IN = 63
    VAR_ROOF001_IN = 183
    VAR_FLOOR001_IN = 159

    records = []  # (month, day, hour, subhour, zone_temp, outdoor_temp)

    current_env = None
    current_month = 0
    current_day = 0
    current_hour = 0
    current_subhour = 0
    in_annual = False
    header_done = False

    current_outdoor = None
    current_zone_temp = None
    current_wall001 = None
    current_roof001 = None
    current_floor001 = None

    with open(eso_path, 'r') as f:
        for line in f:
            line = line.strip()
            if not line:
                continue

            # Skip until we get past the header section (ends at 'End of Data Dictionary')
            if line.startswith('End of Data Dictionary'):
                header_done = True
                continue
            if not header_done:
                continue

            if line.startswith('End of Data'):
                break

            parts = line.split(',')
            try:
                record_id = int(parts[0])
            except ValueError:
                continue

            # Environment record (id=1)
            if record_id == 1:
                env_name = parts[1].strip()
                # We want "RUN PERIOD 1" (the annual simulation, not design days)
                in_annual = 'RUN PERIOD' in env_name.upper()
                continue

            if not in_annual:
                continue

            # Timestep record (id=2)
            if record_id == 2:
                # 2,day_of_sim,month,day,dst,hour,startmin,endmin,daytype
                month = int(parts[2])
                day = int(parts[3])
                hour = int(parts[5])
                start_min = float(parts[6])
                end_min = float(parts[7])

                # Skip hourly summary records (startmin=0, endmin=60)
                is_hourly = (abs(start_min) < 0.01 and abs(end_min - 60.0) < 0.01)
                if is_hourly:
                    # Save accumulated data if any, then skip
                    if current_zone_temp is not None:
                        records.append((
                            len(records),
                            current_month, current_day, current_hour, current_subhour,
                            current_zone_temp, current_outdoor,
                            current_wall001, current_roof001, current_floor001
                        ))
                    current_zone_temp = None
                    current_outdoor = None
                    current_wall001 = None
                    current_roof001 = None
                    current_floor001 = None
                    continue

                current_month = month
                current_day = day
                current_hour = hour

                # E+ reports end-of-interval. For 4 timesteps/hour:
                # Interval 1: 0-15min → subhour=1
                # Interval 2: 15-30min → subhour=2
                # Interval 3: 30-45min → subhour=3
                # Interval 4: 45-60min → subhour=4
                if end_min <= 15.01:
                    current_subhour = 1
                elif end_min <= 30.01:
                    current_subhour = 2
                elif end_min <= 45.01:
                    current_subhour = 3
                else:
                    current_subhour = 4

                # Save prior record
                if current_zone_temp is not None:
                    records.append((
                        len(records),
                        current_month, current_day, current_hour, current_subhour,
                        current_zone_temp, current_outdoor,
                        current_wall001, current_roof001, current_floor001
                    ))

                current_zone_temp = None
                current_outdoor = None
                current_wall001 = None
                current_roof001 = None
                current_floor001 = None
                continue

            # Daily record (id=3) — skip
            if record_id == 3:
                # Save last timestep data before the daily summary
                if current_zone_temp is not None:
                    records.append((
                        len(records),
                        current_month, current_day, current_hour, current_subhour,
                        current_zone_temp, current_outdoor,
                        current_wall001, current_roof001, current_floor001
                    ))
                    current_zone_temp = None
                    current_outdoor = None
                continue

            # Variable data
            if record_id == VAR_OUTDOOR:
                current_outdoor = float(parts[1])
            elif record_id == VAR_ZONE_TEMP:
                current_zone_temp = float(parts[1])
            elif record_id == VAR_WALL001_IN:
                current_wall001 = float(parts[1])
            elif record_id == VAR_ROOF001_IN:
                current_roof001 = float(parts[1])
            elif record_id == VAR_FLOOR001_IN:
                current_floor001 = float(parts[1])

    # Don't forget the last record
    if current_zone_temp is not None:
        records.append((
            len(records),
            current_month, current_day, current_hour, current_subhour,
            current_zone_temp, current_outdoor,
            current_wall001, current_roof001, current_floor001
        ))

    return records

# ── Parse OpenBSE CSV ────────────────────────────────────────────────────────

def parse_openbse(csv_path):
    """Parse OpenBSE results CSV."""
    records = []
    with open(csv_path) as f:
        reader = csv.DictReader(f)
        for row in reader:
            records.append((
                len(records),
                int(row['Month']),
                int(row['Day']),
                int(float(row['Hour'])),
                int(float(row['SubHour'])),
                float(row['ZONE ONE:zone_temp [°C]']),
                float(row['Weather:outdoor_temp [°C]']),
            ))
    return records

# ── Compare ─────────────────────────────────────────────────────────────────

def compare(eplus_records, openbse_records):
    """Compare E+ and OpenBSE zone temperatures."""

    print(f"E+ records: {len(eplus_records)}")
    print(f"OpenBSE records: {len(openbse_records)}")
    print()

    # Build lookup by (month, day, hour, subhour)
    ep_by_time = {}
    for r in eplus_records:
        key = (r[1], r[2], r[3], r[4])
        ep_by_time[key] = r

    ob_by_time = {}
    for r in openbse_records:
        key = (r[1], r[2], r[3], r[4])
        ob_by_time[key] = r

    # Find common timesteps
    common_keys = sorted(set(ep_by_time.keys()) & set(ob_by_time.keys()))
    print(f"Common timesteps: {len(common_keys)}")

    if not common_keys:
        print("ERROR: No common timesteps found!")
        # Print first few of each
        for i, r in enumerate(eplus_records[:5]):
            print(f"  E+ [{i}]: month={r[1]}, day={r[2]}, hour={r[3]}, sub={r[4]}")
        for i, r in enumerate(openbse_records[:5]):
            print(f"  OB [{i}]: month={r[1]}, day={r[2]}, hour={r[3]}, sub={r[4]}")
        return

    # Compute statistics
    diffs = []
    abs_diffs = []
    max_diff = 0
    max_diff_info = None

    for key in common_keys:
        ep = ep_by_time[key]
        ob = ob_by_time[key]
        ep_temp = ep[5]
        ob_temp = ob[5]
        diff = ob_temp - ep_temp
        diffs.append(diff)
        abs_diffs.append(abs(diff))
        if abs(diff) > max_diff:
            max_diff = abs(diff)
            max_diff_info = (key, ep_temp, ob_temp, diff)

    mean_diff = sum(diffs) / len(diffs)
    mean_abs_diff = sum(abs_diffs) / len(abs_diffs)
    rmse = (sum(d**2 for d in diffs) / len(diffs)) ** 0.5

    # E+ annual stats
    ep_temps = [ep_by_time[k][5] for k in common_keys]
    ob_temps = [ob_by_time[k][5] for k in common_keys]

    print()
    print("=" * 70)
    print("  ANNUAL STATISTICS COMPARISON")
    print("=" * 70)
    print(f"  {'Metric':<30s} {'E+':<12s} {'OpenBSE':<12s} {'Diff':<10s}")
    print(f"  {'-'*30} {'-'*12} {'-'*12} {'-'*10}")
    print(f"  {'Annual Min [°C]':<30s} {min(ep_temps):>10.2f}   {min(ob_temps):>10.2f}   {min(ob_temps)-min(ep_temps):>+8.2f}")
    print(f"  {'Annual Max [°C]':<30s} {max(ep_temps):>10.2f}   {max(ob_temps):>10.2f}   {max(ob_temps)-max(ep_temps):>+8.2f}")
    print(f"  {'Annual Mean [°C]':<30s} {sum(ep_temps)/len(ep_temps):>10.2f}   {sum(ob_temps)/len(ob_temps):>10.2f}   {sum(ob_temps)/len(ob_temps)-sum(ep_temps)/len(ep_temps):>+8.2f}")
    print()
    print(f"  {'Mean Bias [°C]':<30s} {mean_diff:>+10.3f}")
    print(f"  {'Mean Abs Error [°C]':<30s} {mean_abs_diff:>10.3f}")
    print(f"  {'RMSE [°C]':<30s} {rmse:>10.3f}")
    print(f"  {'Max |Error| [°C]':<30s} {max_diff:>10.3f}")
    if max_diff_info:
        k, et, ot, d = max_diff_info
        print(f"    at Month={k[0]}, Day={k[1]}, Hour={k[2]}, Sub={k[3]}: E+={et:.2f}, OB={ot:.2f}")

    # Daily swing comparison for key days
    print()
    print("=" * 70)
    print("  DAILY SWING COMPARISON")
    print("=" * 70)
    print(f"  {'Day':<12s} {'E+ Min':>8s} {'E+ Max':>8s} {'E+ Swg':>8s} {'OB Min':>8s} {'OB Max':>8s} {'OB Swg':>8s}")
    print(f"  {'-'*12} {'-'*8} {'-'*8} {'-'*8} {'-'*8} {'-'*8} {'-'*8}")

    for m, d, label in [(1,15,'Jan 15'), (3,21,'Mar 21'), (6,21,'Jun 21'), (7,15,'Jul 15'),
                         (9,21,'Sep 21'), (12,21,'Dec 21')]:
        ep_day = [ep_by_time[k][5] for k in common_keys if k[0]==m and k[1]==d]
        ob_day = [ob_by_time[k][5] for k in common_keys if k[0]==m and k[1]==d]
        if ep_day and ob_day:
            print(f"  {label:<12s} {min(ep_day):>8.2f} {max(ep_day):>8.2f} {max(ep_day)-min(ep_day):>8.2f} "
                  f"{min(ob_day):>8.2f} {max(ob_day):>8.2f} {max(ob_day)-min(ob_day):>8.2f}")

    # Hourly detail for Jul 15
    print()
    print("=" * 70)
    print("  HOURLY DETAIL: Jul 15  (E+ vs OpenBSE zone temp)")
    print("=" * 70)
    print(f"  {'Hour':>4s}:{'Sub':<3s}  {'E+ [°C]':>9s}  {'OB [°C]':>9s}  {'Diff [°C]':>10s}  {'OutdoorT':>9s}")
    print(f"  {'-'*4}:{'-'*3}  {'-'*9}  {'-'*9}  {'-'*10}  {'-'*9}")

    jul15_keys = sorted([k for k in common_keys if k[0]==7 and k[1]==15])
    for k in jul15_keys:
        ep = ep_by_time[k]
        ob = ob_by_time[k]
        ep_t = ep[5]
        ob_t = ob[5]
        # Get outdoor temp from E+ if available
        ep_outdoor = ep[6] if ep[6] is not None else 0.0
        print(f"  {k[2]:>4d}:{k[3]:<3d}  {ep_t:>9.2f}  {ob_t:>9.2f}  {ob_t-ep_t:>+10.3f}  {ep_outdoor:>9.2f}")

    # Monthly RMSE breakdown
    print()
    print("=" * 70)
    print("  MONTHLY RMSE BREAKDOWN")
    print("=" * 70)
    for m in range(1, 13):
        month_diffs = [ob_by_time[k][5] - ep_by_time[k][5] for k in common_keys if k[0]==m]
        if month_diffs:
            m_rmse = (sum(d**2 for d in month_diffs) / len(month_diffs)) ** 0.5
            m_bias = sum(month_diffs) / len(month_diffs)
            m_names = ['Jan','Feb','Mar','Apr','May','Jun','Jul','Aug','Sep','Oct','Nov','Dec']
            print(f"  {m_names[m-1]}: RMSE={m_rmse:.3f}°C, Bias={m_bias:+.3f}°C, N={len(month_diffs)}")


if __name__ == '__main__':
    eso_path = '/Users/benjaminbrannon/Documents/GitHub/New_EnergyPlus/validation/eplus_output/eplusout.eso'
    csv_path = '/Users/benjaminbrannon/Documents/GitHub/New_EnergyPlus/examples/1zone_uncontrolled_results.csv'

    print("Parsing EnergyPlus ESO...")
    ep = parse_eso(eso_path)
    print("Parsing OpenBSE CSV...")
    ob = parse_openbse(csv_path)

    compare(ep, ob)
