#!/usr/bin/env python3
"""
Solar Validation Script for OpenBSE.

Replicates the OpenBSE solar calculation chain (solar.rs) in Python to compare
annual incident and transmitted solar with EnergyPlus reference values from
the ASHRAE 140-2023 results spreadsheet.

Reference values (E+):
  - Incident solar on south face: 1370.5 kWh/m²
  - Transmitted solar Case 600: 804.0 kWh/m² (SHGC=0.789, double-pane clear)
  - Transmitted solar Case 670: 1024.2 kWh/m² (SHGC=0.86, single-pane clear)
  - Transmitted solar Case 660: 436.0 kWh/m² (SHGC=0.44, low-e)
"""

import csv
import math
import os
from datetime import datetime

PI = math.pi
N_GLASS = 1.526  # soda-lime glass refractive index

# ────────────────────────────────────────────────────────────────────────────
# Solar Position (matches solar.rs solar_position)
# ────────────────────────────────────────────────────────────────────────────

def equation_of_time(doy):
    """Equation of time correction [hours]."""
    b = 2.0 * PI * (doy - 81.0) / 364.0
    return 0.1645 * math.sin(2.0 * b) - 0.1255 * math.cos(b) - 0.025 * math.sin(b)


def solar_position(doy, solar_hour, latitude_deg):
    """Calculate solar position. Returns (altitude_rad, azimuth_rad, cos_zenith, is_sunup)."""
    lat = math.radians(latitude_deg)

    # Solar declination — Spencer (1971)
    day_angle = 2.0 * PI * (doy - 1.0) / 365.0
    declination = (0.006918
        - 0.399912 * math.cos(day_angle)
        + 0.070257 * math.sin(day_angle)
        - 0.006758 * math.cos(2.0 * day_angle)
        + 0.000907 * math.sin(2.0 * day_angle)
        - 0.002697 * math.cos(3.0 * day_angle)
        + 0.00148  * math.sin(3.0 * day_angle))

    # Hour angle
    hour_angle = (solar_hour - 12.0) * math.radians(15.0)

    # Solar altitude
    sin_alt = (math.sin(lat) * math.sin(declination)
               + math.cos(lat) * math.cos(declination) * math.cos(hour_angle))
    sin_alt = max(-1.0, min(1.0, sin_alt))
    altitude = math.asin(sin_alt)

    cos_zenith = max(0.0, sin_alt)

    # Solar azimuth (from south, west positive)
    cos_alt = max(0.001, math.cos(altitude))
    sin_azimuth = math.cos(declination) * math.sin(hour_angle) / cos_alt
    cos_azimuth = ((sin_alt * math.sin(lat) - math.sin(declination))
                   / (cos_alt * max(0.001, math.cos(lat))))
    azimuth = math.atan2(sin_azimuth, cos_azimuth)

    is_sunup = altitude > 0.0
    return altitude, azimuth, cos_zenith, is_sunup


# ────────────────────────────────────────────────────────────────────────────
# Incident Solar Components (matches solar.rs incident_solar_components)
# ────────────────────────────────────────────────────────────────────────────

def incident_solar_components(beam_normal, diffuse_horiz, global_horiz,
                               altitude, azimuth, cos_zenith, is_sunup,
                               surface_azimuth_deg, surface_tilt_deg,
                               ground_reflectance, doy):
    """Compute incident solar components on a tilted surface (Hay-Davies)."""
    if not is_sunup or (beam_normal + diffuse_horiz) <= 0.0:
        return 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0  # beam, circ, sky, horiz, ground, total, cos_aoi

    tilt = math.radians(surface_tilt_deg)
    surface_az = math.radians(surface_azimuth_deg - 180.0)

    cos_aoi = max(0.0,
        math.sin(altitude) * math.cos(tilt)
        + math.cos(altitude) * math.sin(tilt)
          * math.cos(azimuth - surface_az))

    # Beam
    i_beam = beam_normal * cos_aoi

    # Hay-Davies diffuse
    vf_sky = (1.0 + math.cos(tilt)) / 2.0
    cos_z = max(0.087, cos_zenith)

    # Extraterrestrial irradiance
    i_ext = 1353.0 * (1.0 + 0.033 * math.cos(2.0 * PI * doy / 365.0))
    a_i = max(0.0, min(1.0, beam_normal / i_ext)) if i_ext > 0.0 else 0.0
    cos_aoi_cs = max(0.0, cos_aoi / cos_z) if cos_z > 0.01 else 0.0

    # Circumsolar
    i_circumsolar = max(0.0, diffuse_horiz * a_i * cos_aoi_cs)

    # Isotropic sky
    i_iso_sky = max(0.0, diffuse_horiz * (1.0 - a_i) * vf_sky)

    # Horizon (not used in CS-only mode)
    i_horizon = 0.0

    # Ground-reflected
    i_ground = global_horiz * ground_reflectance * (1.0 - math.cos(tilt)) / 2.0

    total = max(0.0, i_beam + i_iso_sky + i_circumsolar + i_horizon + i_ground)

    return i_beam, i_circumsolar, i_iso_sky, i_horizon, i_ground, total, cos_aoi


# ────────────────────────────────────────────────────────────────────────────
# Fresnel Optics (matches solar.rs)
# ────────────────────────────────────────────────────────────────────────────

def fresnel_reflectance(cos_theta, cos_theta_p, n):
    rs = ((cos_theta - n * cos_theta_p) / (cos_theta + n * cos_theta_p)) ** 2
    rp = ((n * cos_theta - cos_theta_p) / (n * cos_theta + cos_theta_p)) ** 2
    return (rs + rp) / 2.0


def single_pane_transmittance(cos_theta, n, kd):
    sin_theta = math.sqrt(1.0 - cos_theta * cos_theta)
    sin_theta_p = sin_theta / n
    if sin_theta_p >= 1.0:
        return 0.0
    cos_theta_p = math.sqrt(1.0 - sin_theta_p * sin_theta_p)
    r = fresnel_reflectance(cos_theta, cos_theta_p, n)
    tau_abs = math.exp(-kd / cos_theta_p)
    return tau_abs * (1.0 - r) ** 2 / (1.0 - (r * tau_abs) ** 2)


def single_pane_reflectance(cos_theta, n, kd):
    sin_theta = math.sqrt(1.0 - cos_theta * cos_theta)
    sin_theta_p = sin_theta / n
    if sin_theta_p >= 1.0:
        return 1.0
    cos_theta_p = math.sqrt(1.0 - sin_theta_p * sin_theta_p)
    r = fresnel_reflectance(cos_theta, cos_theta_p, n)
    tau_abs = math.exp(-kd / cos_theta_p)
    return r + r * (tau_abs * (1.0 - r)) ** 2 / (1.0 - (r * tau_abs) ** 2)


def double_pane_transmittance(cos_theta, n, kd):
    t1 = single_pane_transmittance(cos_theta, n, kd)
    r1 = single_pane_reflectance(cos_theta, n, kd)
    if r1 < 1.0:
        return t1 * t1 / (1.0 - r1 * r1)
    return 0.0


def double_pane_reflectance(cos_theta, n, kd):
    t1 = single_pane_transmittance(cos_theta, n, kd)
    r1 = single_pane_reflectance(cos_theta, n, kd)
    if r1 < 1.0:
        return r1 + t1 * t1 * r1 / (1.0 - r1 * r1)
    return 1.0


# ────────────────────────────────────────────────────────────────────────────
# Glass Angular Parameters (matches solar.rs)
# ────────────────────────────────────────────────────────────────────────────

def derive_kd_from_pane_tau(tau_pane, n=N_GLASS):
    """Derive kd from single-pane solar transmittance at normal incidence."""
    r = ((n - 1.0) / (n + 1.0)) ** 2
    a = r * r * tau_pane
    b = (1.0 - r) ** 2
    c = -tau_pane
    disc = b * b - 4.0 * a * c
    if disc < 0.0 or abs(a) < 1e-15:
        return max(0.0, -math.log(tau_pane))
    tau_abs = (-b + math.sqrt(disc)) / (2.0 * a)
    return max(0.0, -math.log(max(0.001, min(1.0, tau_abs))))


def compute_glass_angular_params(shgc, pane_tau=None, pane_rho=None):
    """Compute kd and N_i for angular SHGC model."""
    if pane_tau is not None and pane_rho is not None:
        kd = derive_kd_from_pane_tau(pane_tau)
        tau_sys = double_pane_transmittance(1.0, N_GLASS, kd)
        rho_sys = double_pane_reflectance(1.0, N_GLASS, kd)
        alpha_sys = max(0.0, 1.0 - tau_sys - rho_sys)
        ni = max(0.0, min(1.0, (shgc - tau_sys) / alpha_sys)) if alpha_sys > 0.001 else 0.0
        return kd, ni
    elif shgc >= 0.55:
        # SimpleGlazingSystem correlation
        if shgc < 0.7206:
            tau_est = 0.939998 * shgc**2 + 0.20332 * shgc
        else:
            tau_est = max(0.0, 1.30415 * shgc - 0.30515)
        # Bisection for kd
        lo, hi = 0.0, 3.0
        for _ in range(60):
            mid = (lo + hi) / 2.0
            tau = double_pane_transmittance(1.0, N_GLASS, mid)
            if tau > tau_est:
                lo = mid
            else:
                hi = mid
        kd = (lo + hi) / 2.0
        tau_sys = double_pane_transmittance(1.0, N_GLASS, kd)
        rho_sys = double_pane_reflectance(1.0, N_GLASS, kd)
        alpha_sys = max(0.0, 1.0 - tau_sys - rho_sys)
        ni = max(0.0, min(1.0, (shgc - tau_sys) / alpha_sys)) if alpha_sys > 0.001 else 0.0
        return kd, ni
    else:
        return 0.0, 0.0


# ────────────────────────────────────────────────────────────────────────────
# Angular SHGC Modifiers (matches solar.rs)
# ────────────────────────────────────────────────────────────────────────────

def fresnel_double_pane_modifier(cos_theta, shgc, kd, ni):
    tau_angle = double_pane_transmittance(cos_theta, N_GLASS, kd)
    rho_angle = double_pane_reflectance(cos_theta, N_GLASS, kd)
    alpha_angle = max(0.0, 1.0 - tau_angle - rho_angle)
    shgc_angle = tau_angle + ni * alpha_angle
    if shgc > 0.0:
        return max(0.0, min(1.0, shgc_angle / shgc))
    return 0.0


def polynomial_angular_modifier(c):
    A = [3.5, -8.0, 9.0, -5.0, 1.5]
    c2 = c * c
    c3 = c2 * c
    c4 = c3 * c
    c5 = c4 * c
    return max(0.0, min(1.0, A[0]*c + A[1]*c2 + A[2]*c3 + A[3]*c4 + A[4]*c5))


def angular_shgc_modifier(cos_incidence, shgc, kd, ni):
    c = max(0.0, min(1.0, cos_incidence))
    if c < 0.01:
        return 0.0
    if shgc >= 0.55:
        return fresnel_double_pane_modifier(c, shgc, kd, ni)
    elif shgc <= 0.25:
        return polynomial_angular_modifier(c)
    else:
        blend = (shgc - 0.25) / (0.55 - 0.25)
        clear_mod = fresnel_double_pane_modifier(c, shgc, kd, ni)
        lowe_mod = polynomial_angular_modifier(c)
        return blend * clear_mod + (1.0 - blend) * lowe_mod


def diffuse_shgc_modifier(shgc, kd, ni):
    N = 200
    num = 0.0
    den = 0.0
    for i in range(N):
        theta = (i + 0.5) / N * PI / 2.0
        cos_t = math.cos(theta)
        w = cos_t * math.sin(theta)
        num += angular_shgc_modifier(cos_t, shgc, kd, ni) * w
        den += w
    return max(0.0, min(1.0, num / den)) if den > 0.0 else 0.88


# ────────────────────────────────────────────────────────────────────────────
# Single-pane versions for Case 670
# ────────────────────────────────────────────────────────────────────────────

def single_pane_system_transmittance(cos_theta, n, kd):
    """Single-pane system transmittance (for Case 670)."""
    return single_pane_transmittance(cos_theta, n, kd)


def single_pane_system_reflectance(cos_theta, n, kd):
    """Single-pane system reflectance (for Case 670)."""
    return single_pane_reflectance(cos_theta, n, kd)


def fresnel_single_pane_modifier(cos_theta, shgc, kd, ni):
    """Angular SHGC modifier for single-pane glass."""
    tau_angle = single_pane_transmittance(cos_theta, N_GLASS, kd)
    rho_angle = single_pane_reflectance(cos_theta, N_GLASS, kd)
    alpha_angle = max(0.0, 1.0 - tau_angle - rho_angle)
    shgc_angle = tau_angle + ni * alpha_angle
    if shgc > 0.0:
        return max(0.0, min(1.0, shgc_angle / shgc))
    return 0.0


# ────────────────────────────────────────────────────────────────────────────
# Main: Read weather file, compute annual incident & transmitted solar
# ────────────────────────────────────────────────────────────────────────────

def main():
    weather_path = os.path.join(
        os.path.dirname(os.path.abspath(__file__)), "..",
        "ASHRAE 140-2023 Accompanying FIles/Std140_TF_Files/Normative Materials/725650TY.csv"
    )

    # Denver location from weather file header
    lat = 39.833
    lon = -104.650
    tz = -7.0
    ground_ref = 0.2  # ASHRAE 140 default

    # South-facing vertical surface
    surf_azimuth = 180.0  # due south
    surf_tilt = 90.0      # vertical

    # ─── Case 600: Double-pane clear, SHGC=0.789 ──────────────────────
    # Per-pane properties from YAML: pane_solar_transmittance=0.834, pane_solar_reflectance=0.075
    shgc_600 = 0.789
    kd_600, ni_600 = compute_glass_angular_params(shgc_600, pane_tau=0.834, pane_rho=0.075)
    diff_mod_600 = diffuse_shgc_modifier(shgc_600, kd_600, ni_600)

    # ─── Case 670: Single-pane clear, SHGC=0.86 ──────────────────────
    # NOTE: Case 670 is single-pane! The standard double-pane model won't apply.
    # We'll compute for comparison using the double-pane model (since that's what
    # our code does with SimpleGlazingSystem correlation for SHGC>=0.55 without per-pane)
    shgc_670 = 0.86
    kd_670, ni_670 = compute_glass_angular_params(shgc_670)
    diff_mod_670 = diffuse_shgc_modifier(shgc_670, kd_670, ni_670)

    # ─── Case 660: Low-e, SHGC=0.44 ──────────────────────────────────
    shgc_660 = 0.439  # from YAML
    kd_660, ni_660 = compute_glass_angular_params(shgc_660)
    diff_mod_660 = diffuse_shgc_modifier(shgc_660, kd_660, ni_660)

    print(f"Case 600: kd={kd_600:.6f}, ni={ni_600:.4f}, diff_mod={diff_mod_600:.4f}")
    print(f"Case 670: kd={kd_670:.6f}, ni={ni_670:.4f}, diff_mod={diff_mod_670:.4f}")
    print(f"Case 660: kd={kd_660:.6f}, ni={ni_660:.4f}, diff_mod={diff_mod_660:.4f}")
    print()

    # Verify normal-incidence SHGC reconstruction for Case 600
    tau_sys_0 = double_pane_transmittance(1.0, N_GLASS, kd_600)
    rho_sys_0 = double_pane_reflectance(1.0, N_GLASS, kd_600)
    alpha_sys_0 = 1.0 - tau_sys_0 - rho_sys_0
    shgc_reconstructed = tau_sys_0 + ni_600 * alpha_sys_0
    print(f"Case 600 normal-incidence check:")
    print(f"  tau_sys(0)={tau_sys_0:.6f}, rho_sys(0)={rho_sys_0:.6f}, alpha_sys(0)={alpha_sys_0:.6f}")
    print(f"  SHGC reconstructed = {shgc_reconstructed:.6f} (input = {shgc_600})")
    print()

    # Pre-compute hemispherical average τ for diffuse (Case 600)
    N_int = 200
    tau_num = 0.0
    tau_den = 0.0
    for i in range(N_int):
        theta = (i + 0.5) / N_int * PI / 2.0
        cos_t = math.cos(theta)
        w = cos_t * math.sin(theta)
        tau_num += double_pane_transmittance(cos_t, N_GLASS, kd_600) * w
        tau_den += w
    _tau_diff_600 = tau_num / tau_den if tau_den > 0 else 0.0
    print(f"  Hemispherical avg τ (Case 600): {_tau_diff_600:.6f}")
    print(f"  Hemispherical avg SHGC mod (Case 600): {diff_mod_600:.6f}")
    print(f"  τ(0°) = {tau_sys_0:.6f}, SHGC(0°) = {shgc_600}")
    print(f"  τ_diff / τ(0°) = {_tau_diff_600 / tau_sys_0:.6f}")
    print(f"  SHGC_diff / SHGC(0°) = {diff_mod_600:.6f}")
    print()

    # ─── Read weather file ──────────────────────────────────────────────
    annual_incident = 0.0    # kWh/m²
    annual_trans_600 = 0.0   # kWh/m² (SHGC-based, τ + absorbed-inward)
    annual_tau_600 = 0.0     # kWh/m² (τ-only, just transmittance)
    annual_trans_670 = 0.0
    annual_trans_660 = 0.0
    annual_beam_incident = 0.0
    annual_diff_incident = 0.0
    monthly_incident = [0.0] * 12
    monthly_trans_600 = [0.0] * 12
    monthly_tau_600 = [0.0] * 12
    monthly_beam_600 = [0.0] * 12
    monthly_diff_600 = [0.0] * 12

    with open(weather_path, 'r') as f:
        # Skip header line (location info)
        header_line = next(f)
        # Skip column header line
        col_header = next(f)

        reader = csv.reader(f)
        for row in reader:
            if len(row) < 50:
                continue

            # Parse date/time
            date_str = row[0].strip()
            time_str = row[1].strip()
            month = int(date_str.split('/')[0])
            day = int(date_str.split('/')[1])
            hour = int(time_str.split(':')[0])

            # Day of year
            dt = datetime(1995, month, day)
            doy = dt.timetuple().tm_yday

            # Solar data
            ghi = float(row[4])
            dni = float(row[7])
            dhi = float(row[10])

            # Convert local standard time hour to solar hour
            # TMY hour convention: hour N means the interval ending at hour N
            # So hour 1 means 00:00-01:00, midpoint = 0.5
            # The data represents the value for the hour ending at the given time
            lst_hour = hour - 0.5  # midpoint of the hour
            eot = equation_of_time(doy)
            # Solar time = LST + EoT + 4*(Lst_meridian - Longitude)/60
            # Standard meridian for MST (tz=-7): -105.0
            lst_meridian = tz * 15.0  # = -105.0
            solar_hour = lst_hour + eot + (lst_meridian - lon) / 15.0

            # Solar position
            alt, az, cos_z, is_sunup = solar_position(doy, solar_hour, lat)

            # Incident solar on south vertical surface
            i_beam, i_circ, i_sky, i_horiz, i_ground, i_total, cos_aoi = \
                incident_solar_components(dni, dhi, ghi,
                                          alt, az, cos_z, is_sunup,
                                          surf_azimuth, surf_tilt,
                                          ground_ref, doy)

            # Accumulate incident solar (W/m² × 1 hr = Wh/m²)
            annual_incident += i_total / 1000.0  # kWh/m²
            annual_beam_incident += (i_beam + i_circ) / 1000.0
            annual_diff_incident += (i_sky + i_horiz + i_ground) / 1000.0
            monthly_incident[month - 1] += i_total / 1000.0

            # ─── Case 600 transmitted solar ─────────────────────────────
            if i_total > 0:
                # Match Rust code: circumsolar goes in DIFFUSE bucket
                # (gets hemispherical modifier), not beam
                # See heat_balance.rs lines 815-830:
                #   shaded_beam = components.beam * sunlit  (pure DNI×cos_aoi)
                #   shaded_sky_diffuse = sky + circ*sunlit + horiz
                #   diffuse_total = shaded_sky_diffuse + ground
                beam_component = i_beam  # only pure beam (DNI × cos_aoi)
                diff_component = i_circ + i_sky + i_horiz + i_ground  # circ in diffuse

                # Beam modifier
                beam_mod_600 = angular_shgc_modifier(cos_aoi, shgc_600, kd_600, ni_600)

                # Transmitted (per m²)
                beam_trans_600 = shgc_600 * beam_mod_600 * beam_component
                diff_trans_600 = shgc_600 * diff_mod_600 * diff_component

                annual_trans_600 += max(0, beam_trans_600 + diff_trans_600) / 1000.0
                monthly_beam_600[month - 1] += max(0, beam_trans_600) / 1000.0
                monthly_diff_600[month - 1] += max(0, diff_trans_600) / 1000.0
                monthly_trans_600[month - 1] += max(0, beam_trans_600 + diff_trans_600) / 1000.0

                # Compute τ-only transmitted (just glass transmittance, no absorbed-inward)
                tau_beam = double_pane_transmittance(max(0.01, cos_aoi), N_GLASS, kd_600)
                # Diffuse τ: hemispherically-averaged double-pane transmittance
                # (compute once outside loop for efficiency, but here for clarity)
                tau_beam_trans = tau_beam * beam_component
                # For diffuse, use hemispherical average τ
                # We'll precompute this and store it
                tau_diff_trans = _tau_diff_600 * diff_component
                annual_tau_600 += max(0, tau_beam_trans + tau_diff_trans) / 1000.0
                monthly_tau_600[month - 1] += max(0, tau_beam_trans + tau_diff_trans) / 1000.0

                # ─── Case 670 ──────────────────────────────────────────
                beam_mod_670 = angular_shgc_modifier(cos_aoi, shgc_670, kd_670, ni_670)
                beam_trans_670 = shgc_670 * beam_mod_670 * beam_component
                diff_trans_670 = shgc_670 * diff_mod_670 * diff_component
                annual_trans_670 += max(0, beam_trans_670 + diff_trans_670) / 1000.0

                # ─── Case 660 ──────────────────────────────────────────
                beam_mod_660 = angular_shgc_modifier(cos_aoi, shgc_660, kd_660, ni_660)
                beam_trans_660 = shgc_660 * beam_mod_660 * beam_component
                diff_trans_660 = shgc_660 * diff_mod_660 * diff_component
                annual_trans_660 += max(0, beam_trans_660 + diff_trans_660) / 1000.0

    # ─── Print results ──────────────────────────────────────────────────
    print("=" * 70)
    print("SOLAR VALIDATION: OpenBSE vs EnergyPlus Reference")
    print("=" * 70)
    print()
    print(f"{'Metric':<45} {'OpenBSE':>10} {'E+ Ref':>10} {'Delta':>10} {'%':>8}")
    print("-" * 83)
    print(f"{'Incident solar, south vert [kWh/m²]':<45} {annual_incident:>10.1f} {'1370.5':>10} {annual_incident - 1370.5:>10.1f} {(annual_incident - 1370.5)/1370.5*100:>7.1f}%")
    print(f"{'  Beam+Circumsolar [kWh/m²]':<45} {annual_beam_incident:>10.1f}")
    print(f"{'  Sky+Ground diffuse [kWh/m²]':<45} {annual_diff_incident:>10.1f}")
    print()
    print(f"{'Transmitted C600 SHGC-based [kWh/m²]':<45} {annual_trans_600:>10.1f}")
    print(f"{'Transmitted C600 τ-only [kWh/m²]':<45} {annual_tau_600:>10.1f} {'804.0':>10} {annual_tau_600 - 804.0:>10.1f} {(annual_tau_600 - 804.0)/804.0*100:>7.1f}%")
    print(f"{'  SHGC-based modifier (trans/incident)':<45} {annual_trans_600/annual_incident:>10.4f}")
    print(f"{'  τ-only modifier (trans/incident)':<45} {annual_tau_600/annual_incident:>10.4f} {'0.5866':>10}")
    print(f"{'Transmitted C670 (SHGC=0.86) [kWh/m²]':<45} {annual_trans_670:>10.1f} {'1024.2':>10} {annual_trans_670 - 1024.2:>10.1f} {(annual_trans_670 - 1024.2)/1024.2*100:>7.1f}%")
    print(f"{'Transmitted C660 (SHGC=0.439) [kWh/m²]':<45} {annual_trans_660:>10.1f} {'436.0':>10} {annual_trans_660 - 436.0:>10.1f} {(annual_trans_660 - 436.0)/436.0*100:>7.1f}%")
    print()

    print("─── Monthly Incident Solar (South Vertical) [kWh/m²] ────────")
    month_names = ["Jan","Feb","Mar","Apr","May","Jun","Jul","Aug","Sep","Oct","Nov","Dec"]
    # E+ monthly incident (from Table B9-22)
    ep_monthly_incident = [132.2, 120.7, 130.2, 95.1, 82.2, 67.2, 72.3, 87.4, 116.7, 147.3, 137.2, 118.8]
    # E+ monthly transmitted (from Table B9-22)
    ep_monthly_trans = [80.1, 72.5, 76.3, 51.2, 39.8, 29.0, 33.3, 46.3, 68.5, 91.9, 84.2, 73.4]
    print(f"{'Month':>5} {'OpenBSE':>10} {'E+ Ref':>10} {'Delta':>10} {'%':>8}")
    for i in range(12):
        if i < len(ep_monthly_incident) and ep_monthly_incident[i] > 0:
            delta = monthly_incident[i] - ep_monthly_incident[i]
            pct = delta / ep_monthly_incident[i] * 100
            print(f"{month_names[i]:>5} {monthly_incident[i]:>10.1f} {ep_monthly_incident[i]:>10.1f} {delta:>10.1f} {pct:>7.1f}%")
        else:
            print(f"{month_names[i]:>5} {monthly_incident[i]:>10.1f}")
    total_ep = sum(ep_monthly_incident)
    print(f"{'Total':>5} {annual_incident:>10.1f} {total_ep:>10.1f} {annual_incident - total_ep:>10.1f} {(annual_incident - total_ep)/total_ep*100:>7.1f}%")
    print()

    print("─── Monthly Transmitted C600 [kWh/m²] ──────────────────────")
    print(f"{'Month':>5} {'OpenBSE':>10} {'E+ Ref':>10} {'Beam':>10} {'Diffuse':>10} {'%':>8}")
    for i in range(12):
        if i < len(ep_monthly_trans) and ep_monthly_trans[i] > 0:
            delta = monthly_trans_600[i] - ep_monthly_trans[i]
            pct = delta / ep_monthly_trans[i] * 100
            print(f"{month_names[i]:>5} {monthly_trans_600[i]:>10.1f} {ep_monthly_trans[i]:>10.1f} "
                  f"{monthly_beam_600[i]:>10.1f} {monthly_diff_600[i]:>10.1f} {pct:>7.1f}%")
        else:
            print(f"{month_names[i]:>5} {monthly_trans_600[i]:>10.1f}")
    total_ep_trans = sum(ep_monthly_trans)
    print(f"{'Total':>5} {annual_trans_600:>10.1f} {total_ep_trans:>10.1f}")
    print()

    # ─── SHGC modifier at key angles ───────────────────────────────────
    print("─── Angular SHGC Modifier Curve (Case 600) ─────────────────")
    print(f"{'AOI [deg]':>10} {'cos(AOI)':>10} {'Modifier':>10}")
    for aoi_deg in [0, 10, 20, 30, 40, 50, 60, 70, 80, 85, 90]:
        cos_aoi = math.cos(math.radians(aoi_deg))
        mod = angular_shgc_modifier(cos_aoi, shgc_600, kd_600, ni_600)
        print(f"{aoi_deg:>10} {cos_aoi:>10.4f} {mod:>10.4f}")
    print(f"{'Diffuse':>10} {'':>10} {diff_mod_600:>10.4f}")
    print()

    # ─── Impact analysis ───────────────────────────────────────────────
    print("─── Impact on Heating/Cooling Energy ───────────────────────")
    window_area = 12.0  # 2 x 6 m²
    solar_deficit = (annual_trans_600 - 804.0) * window_area  # kWh
    print(f"Solar transmission deficit (vs E+): {solar_deficit:.0f} kWh")
    print(f"  → Expected heating excess: ~{-solar_deficit * 0.4:.0f} kWh (rough estimate, 40% heating fraction)")
    print(f"  → Actual heating excess: 132 kWh (Case 600: 4636 vs 4504 range max)")
    print(f"  → Actual heating excess vs E+: {4636 - 4324} kWh (Case 600: 4636 vs E+ 4324)")


if __name__ == "__main__":
    main()
