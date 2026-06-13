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
| DX-1/DX-2 wet-coil curves + frozen ADP | **FIXED (round 2)** — full E+ `CalcDoe2DXCoil` ADP/BF method: `tsat_fn_h_pb` added to psychrometrics; ADP recomputed each timestep from current entering conditions and curve-modified capacity; dry coil when w_ADP ≥ w_in. CE100 now matches the committed validated baseline (3303 kWh) and correctly shows zero latent (dry analytical case — the old model spuriously removed 491 kWh latent). |
| M-4 fabricated return-air humidity (found in round 2) | **FIXED** — mixed-air construction used "50% RH at the mixed-air temperature" as return humidity, decoupling coil latent loads from the zone moisture balance and exploding numerically above 100 °C. Now uses the served-zone average humidity ratio via `__return_air_w__` signal. |
| M-5 PLR latch-up against setpoint (found in round 2) | **FIXED** — PSZ heating/cooling capacity was referenced to the setpoint (`cool_sp − supply_temp`); when a capacity-limited coil couldn't push supply past the setpoint during an excursion, PLR latched to 0 and the zone could never recover (CE100 massless zone diverged to 210 °C during warmup). Capacity now referenced to the worse of setpoint and zone temp. |
| HR-1 unbalanced heat recovery | **FIXED (round 2)** — `exhaust_flow_ratio` input (default 1.0 = balanced, schema updated); recovery limited by C_min = ratio·C_supply, latent scaled likewise. |
| HP-3 heuristic defrost | **FIXED (round 2)** — E+ timed-defrost formulation (DXCoils.cc): outdoor-coil T = 0.82·T_odb − 8.589, Δw frost driver, capacity mult 0.909−107.33Δw, input mult 0.90−36.45Δw, LoadDueToDefrost, per-strategy defrost power × runtime fraction. DefrostEIRfT modifier = 1.0 (no curve input yet). `defrost_min_temp` now legacy/unused. |
| P-1 air density (1+w) factor | **FIXED (round 2)** — matched E+ `PsyRhoAirFnPbTdbW` (dropped the (1+w) factor) per the reference-standard policy. All 140 metrics stayed in range; SF gas heating moved to 7387 kWh (+4.8% vs E+ 7052 — now inside the 5% target). |
| M-2 name-substring coil dispatch | **OPEN** — refactor to type-based dispatch (code-review phase). |
| M-3 economizer return temp | **OPEN**. |
| S-4 MRT vs ScriptF | **OPEN** — quantify against E+ before deciding. |
| CTF-1 simple-construction heuristics | **OPEN** — document; prefer layered constructions. |

Round-2 regression results (A/B): case 600 → 4305/5848 kWh (range 3993–4504 / 5432–6162 ✓), case 900 → 1715/2397 (1379–1814 / 2267–2714 ✓), CE100 → 3303.8 kWh elec (validated baseline 3303.3 ✓, latent now correctly 0), SingleFamily Boulder → heating 6247 kWh / gas 7387 kWh (−1.0% vs round 1, now +4.8% vs E+).

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

### Sizing / autosizing (`sizing.rs`) — reviewed 2026-06-13 (Phase 2A)

- **[MED] SIZE-1: Sizing factors applied only to system central capacity, not zone loads or airflows.** `heating_sizing_factor` (default 1.25) / `cooling_sizing_factor` (default 1.15) multiply `coincident_peak_*` for the system coil (sizing.rs:1046-1047), but `run_zone_sizing` receives them as unused `_heating_sizing_factor`/`_cooling_sizing_factor` (sizing.rs:413-414). So zone peak loads, zone design airflows, and `system_airflow` (= Σ zone airflows) carry **no** safety margin, while the central coil capacity does. E+ scales the zone design load by the zone sizing factor, which flows through to zone airflow and the summed system airflow — consistent margin everywhere. Result: airflows / fan / zone-level equipment under-sized ~15-25% vs E+. (Did not bias the validated house's annual energy — capacity-margin mostly affects part-load cycling and unmet hours, not load-driven annual totals — but it is a systematic deviation from E+.) **GitHub #49.**
- **[MED] SIZE-2: Cooling design-day solar uses a generic clear-sky model, not E+'s.** `generate_cooling_design_weather` (sizing.rs:161-169) uses `direct = 1080·exp(−0.174·airmass)`, `diffuse = 120·sin(altitude)`, with `airmass = 1/sin(altitude)`. E+ design days use the **ASHRAE Tau model** (beam/diffuse optical depths `taub`/`taud`, the modern default) or the ASHRAE ClearSky A/B/C model — and the main annual sim uses Kasten air mass, so design-day and run-period solar are computed two different ways. Deviates from E+ on cooling-capacity sizing. **GitHub #50.**
- **[LOW] SIZE-3: Cooling design-day temperature profile is a pure cosine** (`T_max − DR·0.5·(1−cos)`), not E+'s tabulated daily temperature-range-multiplier schedule. Close but not identical near off-peak hours.
- **[LOW] SIZE-4: Heating design-day uses fixed RH 50% and constant horizontal IR (300 W/m²)**; E+ derives sky IR from its sky model. Minor.
- Verified correct: runs ALL design days (not just the first) and takes the max per zone; ideal-loads sizing (exact Q to hold each zone at setpoint) matches E+ zone-sizing intent; coincident system peak summed across zones at the same timestep; warmup-to-quasi-steady before recording peaks; airflow = load / (cp·ΔT_supply) at outdoor density.

### Terminal boxes (`vav_box.rs`, `pfp_box.rs`, `dual_duct_box.rs`) — reviewed 2026-06-13 (Phase 2A)

- **[MED] TERM-1: PFP box uses a static secondary (plenum) air temperature** (`pfp_box.rs:228-250`; `PFPBox::secondary_air_temp` default 24 °C, set only from YAML, never updated at runtime). During heating the parallel fan draws "24 °C plenum air" regardless of the actual zone/return temperature, overstating the warm-air benefit and injecting a spurious sensible term when the zone ≠ 24 °C. E+ draws actual plenum/return air. **GitHub #55.**
- **[LOW] TERM-2: PFP mixed humidity uses the primary stream only** (`pfp_box.rs:239`), ignoring plenum humidity.
- **[LOW] TERM-3: VAV hot-water reheat is a capacity-cap** (`m_w·cp·(T_in − design_outlet)`, `vav_box.rs:244-265`), not a UA/effectiveness coil; adequate but doesn't vary with entering-air temp like E+.
- Verified correct: VAV dual-maximum control (G36 / E+ ReverseWithLimits — cooling min→max, heating min→max_reheat_fraction with reheat), electric reheat COP=1 with delivered energy consistent with the max-reheat-temp clamp; dual-duct CAV box (constant design flow, energy-weighted hot/cold deck blend) matches E+ DualDuct:ConstantVolume. **The previously-flagged `vav_box::tests::test_vav_heating_mode_with_electric_reheat` failure is resolved** (fixed by the typed-control-signals / type-based dispatch work).

### Water-to-air heat pumps (`gshp.rs`, `wshp.rs`) — reviewed 2026-06-13 (Phase 2B) — GitHub #56

- **[MED] HPF-1: GSHP `eir_ft` applied as a COP multiplier** (`power = cap/(cop·eir_mod)`, gshp.rs:366,389), inverse of E+ EIRfT (>1 = worse). Pasting E+ EIRfT coefficients inverts the efficiency response. Default 1.0, so only bites when curves supplied.
- **[MED] HPF-2: GSHP has no part-load cycling penalty** (not in `dx_compressor_names`, no internal PLF) — runs at full COP at any load. WSHP gets the system RTF; GSHP gets nothing.
- **[MED] HPF-3: WSHP condenser temp proxied from outdoor-air temp** (wshp.rs:162), not the actual water loop; only capacity derates, COP constant.
- **[MED] HPF-4: WSHP heating has no source-temperature dependence** (fixed capacity and COP regardless of entering water temp).
- Verified: WSHP water-side energy balance (cooling rejects Q_evap+W, heating absorbs Q_out−W); GSHP Kusuda-Achenbach EWT model + EpwMonthly/Monthly sources; both modulate to meet the exact air load.

### VRF (`vrf.rs`) — reviewed 2026-06-13 (Phase 2B) — GitHub #57

- **[MED] VRF-1: no part-load efficiency curve (EIRfPLR).** Compressor power scales linearly with load (`power = load/COP × eir_mod`, temperature modifier only); E+ VRF's signature part-load efficiency gain (inverter compressors) is absent → likely over-predicts VRF energy at low PLR.
- **[LOW] VRF-2: curve arg order/wet-bulb** — `cap_ft(t_outdoor, avg_indoor_t)` uses (outdoor_db, indoor_db); E+ cooling CapFT is f(indoor_wb, outdoor_db). HP-4 class.
- **[LOW] VRF-3: no piping correction, no defrost.**
- Verified: EIRfT applied in the **correct** direction here (`load/COP × eir_mod`) — opposite of the GSHP bug (#56), confirming conventions are inconsistent across heat-pump components; heat-recovery (route cooling rejection to heating zones) is a defensible simplification.

### Radiant panels + thermal storage (`radiant_panel.rs`, `thermal_storage.rs`) — reviewed 2026-06-13 (Phase 2B) — GitHub #58

- **[MED] RAD-1: radiant output independent of entering water temp** — `q = rated_capacity × plr`; the `ua` field and `entering_water_temp` are unused in the heat calc. A panel fed tepid water still delivers rated output. Matters for low-temp/condensing/outdoor-reset hydronic and chilled-ceiling designs.
- Verified: radiant/convective split routes radiant heat to surfaces/MRT (correct), ASHRAE HOF default fractions. `thermal_storage.rs` structurally sound (SoC = charge − discharge − standby UA·ΔT, ice-charge COP penalty); follow-up: confirm SoC clamped to [0, capacity] and discharge ≤ available charge.

### Plant auxiliaries (`cooling_tower.rs`, `pump.rs`, condenser coupling) — reviewed 2026-06-13 (Phase 2C)

- **CH-1 resolved (verified correct):** the condenser loop is loaded with `thermal + electric` = Q_evap + W_compressor from each chiller referencing it (main.rs:2652-2675), simulated after the CHW loop. Chiller condenser temp comes from a condenser setpoint or wet-bulb + tower approach (E+ SetpointManager-style); CAPFT/EIRFT use f(T_chw_leaving, T_cond_entering) — correct E+ Chiller:Electric:EIR order. Minor: chiller condenser temp is a setpoint/proxy rather than iterated against the tower's actual leaving temp (common simplification).
- **Cooling tower — sound.** Effectiveness-NTU, T_out floored at T_wb + approach, variable-speed fan power via the E+ `CoolingTower:VariableSpeed` polynomial (coefficients match E+). No findings.
- **Pump — sound.** Affinity-law power `P_design·(Q/Q_design)^n`, `P_design = Q·H/η`, motor-loss heat added to fluid; matches E+ Pump:VariableSpeed intent (E+ uses a cubic part-load curve — equivalent). No findings.

### Evaporative cooler (`evap_cooler.rs`) — reviewed 2026-06-13 (Phase 2C) — GitHub #59

- **[MED] EVAP-1: indirect & two-stage drive off the *primary-inlet* wet-bulb, ignoring the secondary (outdoor) stream.** `indirect_stage` (evap_cooler.rs:83-87) computes the wet-bulb limit from the primary stream being cooled, and `simulate_air` takes `_ctx` so `ctx.outdoor_air` is unused. E+ indirect coolers cool the primary stream toward the **secondary/scavenger air** wet-bulb (a separate, usually outdoor stream). Wrong achievable cooling and wet-bulb floor whenever primary inlet ≠ outdoor air (return air, or downstream of mixing/another coil). Same live-coupling pattern as #55/#56/#58. Direct mode is correct (adiabatic saturation of its own stream).
- **[LOW] EVAP-2: indirect effectiveness is the product `effectiveness · hx_effectiveness`** (evap_cooler.rs:85), conflating the direct-pad saturation effectiveness with the indirect HX effectiveness. E+ CelDekPad applies them in series on the two streams (pad cools secondary, then HX transfers between streams); the product form is only equivalent when primary inlet = secondary inlet, compounding EVAP-1.
- **[LOW] EVAP-3: pump power hardcoded** at `self.power = 100.0` W (evap_cooler.rs:128), not configurable and independent of size/mode; indirect/two-stage secondary-air fan power not modeled. E+ has design pump-power (and secondary fan) inputs.
- **[LOW/research] EVAP-4: no make-up water accounting** — direct/two-stage add moisture but never tally evaporated water. E+ reports evaporative water consumption. Only matters if water is metered.
- Verified correct: direct-stage adiabatic enthalpy balance + outlet-W solve (`w_fn_tdb_h`, floored at W_in) and `Tdb_out = Tdb_in − ε·(Tdb_in − Twb_in)` approach (matches E+ direct CelDekPad); two-stage ordering (indirect → direct on intermediate state, lower recomputed wet-bulb) gives a colder outlet than either alone; no EIR/COP curve exists, so the EIR-inversion convention issue (#56/#57) does not apply.

### Steam humidifier (`humidifier.rs`) — reviewed 2026-06-13 (Phase 2C) — GitHub #60

- **[MED] HUM-1: RH setpoint converted at a fixed reference temperature, not the zone moisture balance.** When `w_setpoint == 0`, target = `w_fn_tdb_rh_pb(zone_cooling_setpoint, min_rh_setpoint, p_b)` (humidifier.rs:118-123) — static reference temp + RH — and the humidifier drives the supply node directly to it. E+ `SetpointManager:SingleZone:Humidity:Minimum` back-solves the required supply w from the zone air moisture mass balance, so it adapts to actual zone temp/latent load; the OpenBSE proxy does not. Static-proxy pattern (cf. #55/#56/#58/#59).
- **[LOW] HUM-2: steam sensible heat ignored — outlet held isothermal.** `t_out = t_db` adds moisture at the inlet *vapor* enthalpy; steam actually enters at ~100 °C, so the air should gain ~Δw·130 kJ/kg extra sensible (~0.6 °C rise for Δw = 0.005). Electric energy billing unaffected (charged at full `H_STEAM_TOTAL`); only outlet temp slightly under-predicted.
- **[LOW] HUM-3: no standby/fan power, no make-up water accounting** (cf. #59 EVAP-4).
- Verified correct: `H_STEAM_TOTAL = 2,615,700 J/kg` = cp_water·(100 − 14.4 °C) + h_fg(100 °C) ≈ 2,615,322 — exactly the E+ steam-electric energy basis; linear part-load `power = rated_power·(moisture_actual/rated_capacity)` matches E+; capacity clamp (unmet when limited); off when `w_in ≥ w_target`; no EIR curve (convention issue N/A).

### DHW water heater (`water_heater.rs`) — reviewed 2026-06-13 (Phase 2C) — GitHub #61

- **[MED] WH-1: explicit-Euler full-timestep integration, no within-step cycling.** `delta_t = (q_input − q_delivered − q_loss)·dt/(m·Cp)` (water_heater.rs:196) fires full rated capacity for the whole step then jumps T by forward Euler. E+ WaterHeater:Mixed solves the tank ODE analytically and computes the heater on-fraction (shuts off when the tank reaches setpoint mid-step). Coarse timesteps overshoot the setpoint and overstate the firing-step energy; net roughly conserved across steps but per-step T and burner energy aren't E+-consistent. Small at the sub-hourly steps the validated house uses.
- **[MED] WH-2: HPWH uses a constant COP and ignores zone-air coupling.** `HeatPump` treats `efficiency` as a fixed COP with no evaporator-air/tank-temp dependence, and never loads the surrounding zone. E+ WaterHeater:HeatPump has COP curves and removes sensible heat + moisture from the zone (a real cooling/dehumidification load); real HPWH COP also falls as the tank nears setpoint. Live-coupling pattern (cf. #56).
- **[LOW] WH-3: tank ambient temp is a static field** (default 20 °C, never updated), so standby losses use a fixed ambient regardless of location/season. E+ can tie it to a zone/schedule. Static-proxy pattern (#55/#56/#58/#59/#60).
- Verified correct: mixed-tank balance `q_input·eff − q_delivered − q_loss` matches E+ WaterHeater:Mixed; standby loss `UA·(T_tank − T_amb)`; draw energy `m·Cp·(T_tank − mains)` floored at 0; delivers at tank temp when capacity-limited; deadband hysteresis (fire below setpoint−deadband, off at setpoint); `Modulate` control = E+ "Modulate" intent; parasitic = pure waste every step (equal on/off-cycle, zero tank fraction); fuel vs electric routing and gas-eff/electric-COP handling.

### CRAC/CRAH data-center cooling (`airloop/lib.rs:451-650`, `main.rs:2499-2530/3705-3725`, `DataCenterConfig`) — reviewed 2026-06-13 (Phase 2C) — GitHub #62

Compared vs E+ `ElectricEquipment:ITE:AirCooled` (E+ has no dedicated CRAC/CRAH object). This path was bug-fixed in v0.5.1 (lower risk); findings are modeling-fidelity vs E+.

- **[MED] CRAC-1: hot-aisle air rise uses the CRAC/HVAC supply flow, not the IT flow.** `t_hot = t_supply + Q_IT/(m_supply·cp)` (main.rs:2521,3711) uses the room HVAC mass flow to carry IT heat; server fans set the IT airflow independently. E+ ITE models IT flow separately (design flow + fan/CPU curves) and derives supply/return *approach* temps. `t_hot` (and the coil-driving return temp) is only correct when CRAC flow = IT flow; bypass/recirculation breaks it.
- **[MED] CRAC-2: constant-volume fan, no airflow modulation.** `loop_plr = 1.0` and full `total_flow` always (lib.rs:548-553,642-648); only the coil SAT modulates. Real CRAH units modulate fan speed (~cube law); constant full-flow overstates fan energy at part IT load — a dominant DC energy term.
- **[LOW] CRAC-3: containment is a fixed-fraction blend** `t_return = η·t_hot + (1−η)·t_zone` (constant `containment_efficiency`), not E+ ITE recirculation/approach curves. Defensible simplification.
- **[LOW] CRAC-4: dead aisle-config fields** — `rack_outlet_temp_c` and `airflow_m3_per_s_per_kw` are defined/documented in `DataCenterConfig` but never read; `t_hot` comes purely from live supply flow + IT power, so setting them has no effect. (`lighting_w_per_m2` is wired in — input.rs:3863.)
- **[LOW] CRAC-5: return-temp lag** — `zone_dc_return_temps` computed once before the HVAC iteration loop from the previous step's supply temp; doesn't update across sub-iterations.
- Verified correct: no OA mixing / no economizer (`oa_fraction = 0`); cooling-only mode mapping; deadband coil-off (SP 99); RH-driven dehumidification override (cool to zone−0.5); ASHRAE A1–A4 rack-inlet limits (32/35/40/45 °C) and SAT = rack_inlet_max; `Q = m·cp·ΔT` form internally consistent; CRAH→CHW coil (inherits chiller review), CRAC→DX coil (inherits DX-1/DX-2). The "SHR ≈ 0.98" in the builder doc-comment is **not** set by the builder — it relies on the coil's `rated_shr` (doc clarification worthwhile).

## Phase 2D — core numerics & I/O edges (reviewed 2026-06-13)

### AirflowNetwork (`airflow_network.rs` + `heat_balance.rs:1959-1983`) — GitHub #63

AFN is opt-in (`airflow_network: enabled: true`) and not used by the validated house.

- **[MED, bug] AFN-1: Swami-Chandra Cp side-ratio terms use the raw side ratio instead of `ln(side ratio)`.** The published correlation uses G = ln(SR) in `0.131·sin³(2·G·θ)` and `0.07·G²·sin²(θ/2)`; the code (airflow_network.rs:272-276) computes `side_ratio.ln().exp()` (an identity → SR) and `side_ratio²`. Verified numerically: spurious nonzero terms for square buildings (G should be 0), and ~8× error for SR=4. The `.ln().exp()` no-op betrays an intended `ln()`. Existing tests only check θ=0/180 where sin terms vanish, so they miss it.
- **[MED, bug] AFN-2: inconsistent AFN integration — nat-vent double-counted, exhaust missing.** With AFN on, nat-vent openings are added as `LargeOpening` paths (→ `infiltration_mass_flow`) **and** re-added via the separate ASHRAE nat-vent model (`zone.nat_vent_mass_flow`, heat_balance.rs:2104-2122, 3588…) → double count. Exhaust-fan paths are built as `FixedFlow{0.0}` and never updated (heat_balance.rs:1969-1970 comment unimplemented), so mechanical exhaust is absent from the pressure solve and is instead bolted on via the ASHRAE-combined quadrature — defeating the network's main advantage (exhaust↔leakage coupling).
- **[LOW] AFN-3:** crack density correction omits E+ viscosity term `(μ_0/μ)^(2n-1)`; large openings are single-direction orifices (no E+ two-way DetailedOpening neutral-plane model); `MIN_DP` floor vs E+ laminar transition near ΔP→0; cosmetic dead arithmetic (airflow_network.rs:400) and fixed `w=0.008` node density.
- Verified correct: power-law mass-flow density correction `(ρ/ρ_ref)^(1-n)` (→ ρ^0.5 orifice limit); orifice eq + analytical derivative (FD-checked); stack pressure with actual node densities and correct sign/direction; wind pressure `0.5ρCpV²` at outdoor node with terrain height profile, correct sign both orientations; Newton-Raphson Jacobian (FD-checked), Gaussian elimination + partial pivoting + singular detection, damping, dual convergence; mass conservation (tests); live per-timestep zone node temp/density update (stack effect not frozen); windward Cp≈0.6, leeward negative.

### HAMT moisture solver (`hamt.rs`) — GitHub #64

Opt-in: activates only when all layers have `vapor_resistance_factor` + `sorption_isotherm` (else CTF). No validated model supplies that data. As-is the moisture solver is quantitatively non-functional — three coupled bugs:

- **[MED, bug] HAMT-1: latent heat/moisture coupling term is identically zero.** `w_prev` and `w_curr` both read `layer.moisture[ni]` (hamt.rs:369-371), so `latent ≡ 0`. The heat eq's `L_v·∂(ρ₀·w_c)/∂t` source — the whole point of coupled HAMT — is dead; heat solve is plain conduction.
- **[MED, bug] HAMT-2: vapor conductance drops the isotherm slope.** Pa→w_c conversion uses `perm·p_sat` (hamt.rs:510,530), omitting `∂RH/∂w_c` (comment at :490 acknowledges it). Implicitly assumes RH=w_c; mis-scales vapor transport by ~5–50×.
- **[MED, bug] HAMT-3: surface resistance applied as `DELTA_AIR/Z_M` not `1/Z_M`.** Spurious `DELTA_AIR≈2e-10` factor (hamt.rs:489,515,598) makes surface permeance ~2e-10× correct (6.6e-19 vs 3.3e-9), ~9 orders below the interior face — surfaces are effectively vapor-tight, `moisture_flux_inside` ~2e-10× too small. Test only checks flux *sign*, so it passes.
- **[LOW] HAMT-4:** dry (constant) conductivity vs E+ λ(w); Magnus `p_sat` over water (no ice branch) vs Hyland–Wexler elsewhere; uniform node spacing (no near-surface refinement); hard-coded film coefficients (25/8.3).
- Verified correct: TDMA solver; interp/sorption interpolation; harmonic-mean **heat** face conductance; backward-Euler storage `ρ·c_p·dx/dt`; activation gating (all-layers moisture data → else CTF); Magnus `p_sat` values at 0/20 °C. **Treat HAMT as not production-ready until HAMT-1/2/3 fixed.**

### openbse-a205 interpolation (`interpolate.rs` + RS loaders) — GitHub #65

Engine is fundamentally sound; gaps are ASHRAE 205 feature-completeness, not numerical errors.

- **[LOW] A205-1: extrapolation is always constant (edge-clamp).** 205 allows per-grid-variable `linear`/`constant` extrapolation with limits; OpenBSE always clamps. Mitigated by the `in_range` flag computed per query and emitted as a per-component output variable (visible in CSV).
- **[LOW] A205-2: linear (N-linear) interpolation only.** 205 permits `cubic` (monotone Hermite) per axis; not implemented. Linear is exact at grid points but less smooth between.
- Verified correct: N-linear over 2^n corners with product weights (cube center = mean of 8 corners); exact recovery at grid points; degenerate single-point axes ignored; row-major strides with **last axis fastest** matching the 205/btwxt C-order convention; loaders build axes in `grid_variables` declaration order and validate lookup length against the grid product; strict monotonic-axis/arity/size checks; binary-search `locate` with correct fraction + edge clamp; pragmatic `sensible ≤ total` clamp.

## Not yet reviewed (Phase 2 backlog — tracked in GitHub)

`airflow_network.rs`, `hamt.rs`, `openbse-a205` interpolation, shading polygon clipping internals, schedule resolution, weather parsing edge cases. (Phase 2A `sizing.rs` and terminal boxes, Phase 2B heat-pump/radiant/storage equipment, and Phase 2C plant auxiliaries + evap cooler/humidifier/water heater/CRAC-CRAH all reviewed 2026-06-13, see above — Phase 2D core numerics & I/O edges remain.)

### Still-open findings carried to GitHub issues (2026-06-13)

- **S-4 / view-factor approximation (GitHub #52):** interior longwave exchange now uses geometric view factors for zones with vertex geometry, but those view factors are computed from the zone's **rectangular bounding box** — non-rectangular footprints (L-shapes, curved-as-segment walls, atria) are approximated by their enclosing rectangle, and zones with no vertex geometry fall back to area-weighted MRT.
- **M-3 / return-air lighting heat (GitHub #51):** the lights `return_air_fraction` portion of heat is dropped from the thermal balance entirely (electricity still billed) rather than delivered to the cooling coil; the economizer also uses zone temp rather than return-air temp. No effect when `return_air_fraction = 0` (all current validated models); under-counts cooling load when > 0.
- **CTF-1 (GitHub #53):** `calculate_ctf_simple` synthesizes a layer buildup for "simple construction" inputs, tuned to ASHRAE 140 walls; users should prefer explicit layered constructions for unusual assemblies.
- **Phase 2 review backlog tracked in GitHub #54.**

## Recommended fix order

1. **S-1** solar time sign (one-line fix ×2, re-run ASHRAE 140 + E+ comparison to confirm Denver insensitivity)
2. **M-1 / HP-1 / HP-2** cycling-penalty accounting (use ComponentKind; single penalty level)
3. **DX-1 / DX-2** wet-coil capacity vs curves (E+ ADP recomputation)
4. **HR-1** unbalanced-flow heat recovery
5. **DX-4** multi-speed coil consistency
6. **M-2** type-based coil dispatch
7. Docs: S-2, S-3, C-1, P-1 decision (keep or match E+ density), DX-3 warning, HP-4 convention

