# OpenBSE Physics Review — June 2026

Domain-by-domain physics review against the EnergyPlus reference standard.
Severity: **HIGH** (wrong results in common cases) / **MED** (wrong in some cases or systematic deviation from E+) / **LOW** (minor/cosmetic/doc).

## Fix status (2026-06-12)

| Finding | Status |
|---|---|
| S-1 solar time sign | **FIXED** — shared `solar::local_solar_hour()` helper, both call sites; NOAA Boston almanac unit test added; `.solar` cache version bumped to 2. A/B verified: case 600 moves 1 kWh (0.02%), single-family Boulder +0.03% heating — Denver-area validation effectively unchanged, as predicted. |
| M-1 / HP-2 electric resistance billed at RTF | **FIXED** — `LoopInfo.dx_compressor_names` (DX single/multi-speed, WSHP) replaces the `fuel==0` heuristic; heating coils now get PLR. HP heating coil keeps its internal PLF (E+-style), no longer doubled by system RTF (fixes HP-1). |
| DX-3 constant-SHR latent dropped | **FIXED** — total = sensible/SHR, latent removed from airstream (non-default mode only). |
| DX-4 multi-speed inconsistency | **FIXED** — sensible/total/latent/power all scale with stage PLR; derate constants unified with single-speed (0.008 cap, 0.012 EIR). |
| S-2/S-3/C-1 doc errors | **FIXED** — README now says Perez 1990 + Clark & Allen; comments corrected. |
| DX-1/DX-2 wet-coil curves + frozen ADP | **OPEN** — attempted capacity rescaling was reverted after ASHRAE 140 CE100 A/B showed the frozen ADP makes the BF-derived SHR unreliable for dry inlets (sensible/latent split corrupted). Needs E+'s per-timestep ADP/BF recomputation; warning comment added at the site. |
| HR-1 unbalanced heat recovery | **OPEN** — needs exhaust flow plumbed into the component (input schema addition). |
| HP-3 heuristic defrost | **OPEN** — implement E+ reverse-cycle defrost formulation. |
| M-2 name-substring coil dispatch | **OPEN** — refactor to type-based dispatch (code-review phase). |
| M-3 economizer return temp | **OPEN**. |
| P-1 air density (1+w) factor | **OPEN — decision needed**: keep (more physically correct) or match E+ `PsyRhoAirFnPbTdbW` (reference-standard policy). ~1% systematic effect on mass flows. |
| S-4 MRT vs ScriptF | **OPEN** — quantify against E+ before deciding. |
| CTF-1 simple-construction heuristics | **OPEN** — document; prefer layered constructions. |

## Findings

### Psychrometrics (`crates/openbse-psychrometrics/src/lib.rs`) — reviewed, solid

- **[MED] P-1: `rho_air_fn_pb_tdb_w` deviates from E+.** E+ `PsyRhoAirFnPbTdbW` = `pb / (287.0 · T_K · (1 + 1.6078·w))`; OpenBSE multiplies by an extra `(1+w)` (lib.rs:296). OpenBSE's form is total moist-air density (arguably more correct), but it is a ~+1% systematic deviation at w=0.01 from the declared reference everywhere density converts volumetric to mass flow.
- **[LOW] P-2: `W_MIN=1e-5` floor inside `h_fn_tdb_w` / `cp_air_fn_w`** gives dry air a tiny spurious latent enthalpy (~25 J/kg). Negligible.
- **[LOW] P-3: `CP_WATER=4180` constant** (no temperature dependence) vs E+ glycol property routines. Acceptable simplification; matters slightly for low-temp chiller loops vs condenser loops.
- Verified correct: Hyland–Wexler ice + liquid coefficients, `h_fn_tdb_w` constants (1.00484e3/1.85895e3/2.50094e6), specific volume, tsat/twb iterative solvers, RH round-trips.

### Convection (`crates/openbse-envelope/src/convection.rs`) — reviewed, solid

- **[LOW] C-1: Doc comment at convection.rs:147** says `1.5863 = (370/10)^0.22`; actually `(270/10)^0.14`. Constant is correct, comment wrong.
- Verified correct: TARP natural convection coefficients & stability logic (interior negated cosTilt, exterior not), MoWiTT windward 3.26·V^0.89 / leeward 3.55·V^0.617, DOE-2 roughness multipliers, windward 100° threshold, terrain wind profile constants, ISO 15099 window interior convection.

### Zone air balance (`crates/openbse-envelope/src/zone.rs`) — reviewed, solid

- Verified correct: BDF1/2/3 effective-dt formulation (cap_mult 3/2 and 11/6; t_eff (4T−T₂)/3 and (18T−9T₂+2T₃)/11), zone temp solve form matching E+ predictor-corrector, moisture balance with h_fg=2.501e6, ideal loads Q computation (algebraically exact inverse of the solve).

### Solar (`solar.rs`) and heat balance (`heat_balance.rs`)

- **[HIGH] S-1: Solar time longitude correction is sign-flipped.** Both heat_balance.rs:1588 and heat_balance.rs:1810 compute `solar_hour = clock + (time_zone − longitude/15) + EOT`. With EPW conventions (east-positive longitude, e.g. Denver −104.65 / tz −7), the correct correction is `+ longitude/15 − time_zone`. The error is 2× the site's offset from its time-zone meridian: ~3 min for Denver (invisible to ASHRAE 140), ~30 min for Boston, up to ~1 h at zone edges. Shifts all beam solar/shading timing; biases east vs west façade gains. Both sites use the same flipped sign, so shading precompute and main solar at least agree with each other.
- **[LOW] S-2: README/docs say "Hay-Davies" and "Berdahl-Martin"** but the code implements Perez 1990 (correct, matches E+ AnisoSkyViewFactors — coefficients verified) and Clark & Allen sky emissivity (correct, matches E+ default). Docs should be updated, not code.
- **[LOW] S-3: comment/code mismatch** heat_balance.rs:3201-3209: comment says window gap-model U capped at 110% of rated, code uses 1.05 (5%).
- Verified correct: Spencer declination & azimuth formulas; E+ sunup threshold −0.8333°; Perez F11–F23 tables, epsilon bins, Kasten air mass with elevation correction, a/b clamps (cos85°); Clark & Allen ε_clear=0.787+0.764·ln(Tdp/273) with E+ cubic cloud factor; exterior LW split HSky/HAir/HGround with SurfAirSkyRadSplit=sqrt(0.5(1+cosΣ)) matching E+; exact-linearization h_rad (ΔT>0.1 quartic ratio form); outside-surface CTF-coupled balance matching E+ HeatBalanceSurfaceManager; interzone pairing uses paired surface inside temp (E+ approach); window 3-node glass balance with ISO 15099 gap model.
- **[MED] S-4 (approximation, by design): interior LW exchange uses MRT linearization** (per-face view-factor weighted where geometry allows) instead of E+'s ScriptF/Carroll exact matrix exchange. Documented in code; contributes to residual envelope deviation vs E+ in zones with large surface-temperature spread (e.g., big windows + heavy slab). Worth quantifying in the E+ comparison before deciding to upgrade.

### CTF (`ctf.rs`) — reviewed

- Verified: Seem state-space construction (node discretization dx=√(2αΔt), min 6 nodes; interface half-cap nodes; NoMass folded into boundary conductance), Taylor matrix exponential with scaling/squaring, U-value self-check diagnostic, lumped-RC and NaN fallbacks.
- **[MED] CTF-1: `calculate_ctf_simple` invents layer structures** (plasterboard/fiberglass/wood-siding splits, mass caps 10–20%, R thresholds at 10/15/20) heavily tuned to ASHRAE 140 constructions. For user "simple construction" inputs outside that envelope (e.g., heavy uninsulated mass walls, EIFS, SIPs), the synthetic layering — not the user's U and C — controls transient response. Not a bug, but an under-documented modeling risk; users should prefer layered constructions for anything unusual.

### Infiltration (`infiltration.rs`) — reviewed, solid

- Verified: E+ Design Flow Rate model `Q_design·(A + B·|ΔT| + C·V + D·V²)·schedule`, mass flow at outdoor density (matches E+), negative-factor clamp.

### DX cooling coil (`cooling_coil.rs`)

- **[MED] DX-1: Wet-coil capacity ignores performance curves and outdoor conditions.** In the default `autocalculate_shr` wet-coil branch (cooling_coil.rs:399-436), full-load capacity comes purely from the rated-condition ADP/BF geometry applied to current entering air: `q_total_full = ṁ·(h_in − h_out_full)`. The curve-modified `available_cap` (Cap-fT, Cap-fFlow) is only used for the *power* PLR, never to limit or set delivered capacity. So a hot 40 °C day does not derate delivered wet-coil capacity the way E+ does (E+ recomputes ADP each timestep so outlet state is consistent with TotCap·CapFT). Result: oversized apparent capacity at high ODB, and energy non-conservation between "capacity" and curve-derated power normalization.
- **[MED] DX-2: ADP/bypass factor frozen at ARI rated entering conditions.** E+ holds BF≈constant but re-derives ADP from current entering conditions + current total capacity each timestep; OpenBSE freezes ADP itself (t/w/h at rated), so the SHR response to entering wet-bulb deviates from E+ off-rated.
- **[LOW] DX-3: `autocalculate_shr: false` mode drops latent entirely** (q_total = q_sensible, w passes through), under-counting compressor power by ~1/SHR whenever dehumidification would occur. Non-default; worth a doc warning.
- **[MED] DX-4: Multi-speed DX is internally inconsistent** (cooling_coil.rs:775-798): reports `cooling_rate = full stage capacity` and full-capacity power even when sensible delivery is clipped to the load; removes latent moisture at the full-capacity rate regardless of sensible clipping (outlet enthalpy change ≠ reported total); uses different hard-coded derate slopes (0.007/°C cap, 0.003/°C EIR) than the single-speed defaults (0.008/0.012). STATUS lists multi-speed as in-progress — recommend finishing before use.

### Heat pump coil (`heat_pump_coil.rs`)

- **[MED] HP-1: Cycling penalty applied twice for PTHP.** The coil applies PLF internally (`runtime = plr/plf` at heat_pump_coil.rs:~300) in `compressor_power`, and main.rs *also* multiplies the component's electric power by system RTF for PLR-cycling systems (PSZ/PTAC/PTHP, main.rs:5370-5374). When the coil's internal PLR < 1, the compressor gets two cycling penalties. Pick one level (the system level, per the established convention for DX cooling) and make the coil report unpenalized power.
- **[MED] HP-2: Supplemental electric resistance heat gets a compressor cycling penalty.** `power_consumption()` returns compressor + defrost + supplemental in one number; main.rs's `is_dx_coil` heuristic (electric, non-fan ⇒ DX) multiplies all of it by RTF. Electric resistance has no PLF degradation in E+.
- **[MED] HP-3: Defrost model is heuristic, not the E+ formulation.** E+ reverse-cycle defrost: `q_defrost = 0.01·f_def·(7.222 − T_odb)·(Q_rated/1.01667)` with a defrost EIR curve; OpenBSE uses a linear capacity ramp and a flat "0.3 × defrost fraction" power adder. Directionally right, quantitatively unvalidated.
- **[LOW] HP-4: Curve argument-order inconsistency.** HP coil evaluates `cap_ft(T_outdoor, T_indoor)`; DX cooling evaluates `cap_ft(T_wb_indoor, T_outdoor)`. E+ heating-DX biquadratics are f(T_indoor, T_outdoor). Anyone pasting E+ curve coefficients into the HP coil gets swapped axes. Document or unify.

### System control / energy accounting (`openbse-cli/src/main.rs`)

- **[MED] M-1: `is_dx_coil` heuristic misclassifies electric resistance coils** (main.rs:5370-5372): any non-fan component with zero fuel power is treated as a DX compressor and billed at RTF instead of PLR — inflates electric-resistance heating energy by up to ~8% at PLR 0.5 on cycling systems. Should use `ComponentKind` instead of the fuel==0 heuristic.
- **[MED] M-2: Coil control dispatch by name substring** (main.rs:6837-6855): `lname.contains("cool")/"heat")` etc. A coil named outside these patterns silently receives no setpoint; a "Precooling heat exchanger" matches both. Silent physics-wrong behavior from a naming choice — should dispatch on component type.
- **[LOW] M-3: Economizer uses average zone temp as return-air temp** for both the high-limit comparison (DifferentialDryBulb) and mixing. E+ uses actual return-air temperature (includes return-fraction lighting heat). Small bias toward less economizing.
- Verified: system-level PLF formulation `PLF = 1 − 0.15(1−PLR)`, RTF=PLR/PLF, fuel and fan at PLR (matches E+ convention); economizer high-limit types match 90.1 options; LockoutWithHeating logic documented; continuous-fan averaged supply temp model is sound.

### Chiller / boiler / fan / HX (spot-checked)

- Verified: chiller power = AvailCap·(1/COP_ref)·EIRfT·EIRfPLR·CyclingRatio (E+ Chiller:Electric:EIR form), min-PLR cycling ratio; fan power = ṁ·ΔP/(η_tot·ρ) with E+ motor-heat split; Kusuda-Achenbach formula; boiler efficiency-curve structure.
- **[MED] HR-1: Heat recovery assumes balanced flows.** `q = ε·C_supply·(T_exh − T_OA)` (heat_recovery.rs:162) regardless of exhaust-side capacity rate. When exhaust flow < supply flow (common — exhaust is usually 80-90% of OA), recovery is overestimated; comment claims this is "conservative" but it's the opposite. E+ scales effectiveness with the flow ratio. No frost-control either.
- **[LOW] CH-1: Chiller condenser heat rejection** isn't added to the condenser water stream inside `simulate_plant` (outlet is evaporator side only); verify the condenser-loop coupling path handles Q_cond = Q_evap + P elsewhere.

## Not yet reviewed (suggest phase 2)

VRF, GSHP, WSHP, radiant panel, thermal storage, evap cooler, humidifier, water heater, cooling tower internals, pumps, dual-duct/PFP/VAV boxes (note: `vav_box` has a known pre-existing test failure), CRAC/CRAH details (recently bug-fixed in v0.5.1), `airflow_network.rs`, `hamt.rs`, `sizing.rs`, `openbse-a205` interpolation, shading polygon clipping internals, schedule resolution, weather parsing edge cases.

## Recommended fix order

1. **S-1** solar time sign (one-line fix ×2, re-run ASHRAE 140 + E+ comparison to confirm Denver insensitivity)
2. **M-1 / HP-1 / HP-2** cycling-penalty accounting (use ComponentKind; single penalty level)
3. **DX-1 / DX-2** wet-coil capacity vs curves (E+ ADP recomputation)
4. **HR-1** unbalanced-flow heat recovery
5. **DX-4** multi-speed coil consistency
6. **M-2** type-based coil dispatch
7. Docs: S-2, S-3, C-1, P-1 decision (keep or match E+ density), DX-3 warning, HP-4 convention

