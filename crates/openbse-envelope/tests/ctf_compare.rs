#[cfg(test)]
mod ctf_compare {
    use openbse_envelope::ctf::{calculate_ctf, calculate_ctf_simple};
    use openbse_envelope::material::ResolvedLayer;

    #[test]
    fn test_ctf_compare_wall() {
        let dt: f64 = 900.0;
        // E+ explicit layers for HWWALL: Wood Siding | Foam | Concrete Block
        let ep_layers = vec![
            ResolvedLayer::new(0.14, 530.0, 900.0, 0.009),
            ResolvedLayer::new(0.04, 10.0, 1400.0, 0.0615),
            ResolvedLayer::new(0.51, 1400.0, 1000.0, 0.100),
        ];
        let ep_ctf = calculate_ctf(&ep_layers, dt);

        // Our current simplified 2-layer model
        let our_ctf = calculate_ctf_simple(0.556, 145154.0, dt, false, Some(0.51), Some(1400.0));

        // Proposed 3-layer: [wood_siding, insulation, concrete]
        let k_fin: f64 = 0.14;
        let rho_fin: f64 = 530.0;
        let cp_fin: f64 = 900.0;
        let t_fin: f64 = 0.009;
        let cap_fin: f64 = rho_fin * cp_fin * t_fin;
        let k_mass: f64 = 0.51;
        let rho_mass: f64 = 1400.0;
        let cp_mass: f64 = 1000.0;
        let cap_total: f64 = 145154.0;
        let cap_mass: f64 = cap_total - cap_fin;
        let t_mass: f64 = cap_mass / (rho_mass * cp_mass);
        let r_total: f64 = 1.0 / 0.556;
        let r_fin: f64 = t_fin / k_fin;
        let r_mass: f64 = t_mass / k_mass;
        let r_insul: f64 = (r_total - r_fin - r_mass).max(0.01);
        let k_insul: f64 = 0.04;
        let t_insul_raw: f64 = k_insul * r_insul;
        let max_insul_t: f64 = 0.1 * cap_total / (10.0 * 1000.0);
        let t_insul: f64 = t_insul_raw.min(max_insul_t).max(0.001);
        let actual_k_insul: f64 = if t_insul < t_insul_raw {
            t_insul / r_insul
        } else {
            k_insul
        };
        let proposed_layers = vec![
            ResolvedLayer::new(k_fin, rho_fin, cp_fin, t_fin),
            ResolvedLayer::new(actual_k_insul, 10.0, 1000.0, t_insul),
            ResolvedLayer::new(k_mass, rho_mass, cp_mass, t_mass),
        ];
        let proposed_ctf = calculate_ctf(&proposed_layers, dt);

        println!("\n=== WALL Z[0] (interior self-response) ===");
        println!("E+ Z[0]:       {:.4}", ep_ctf.z[0]);
        println!(
            "Current Z[0]:  {:.4} (ratio: {:.4})",
            our_ctf.z[0],
            our_ctf.z[0] / ep_ctf.z[0]
        );
        println!(
            "Proposed Z[0]: {:.4} (ratio: {:.4})",
            proposed_ctf.z[0],
            proposed_ctf.z[0] / ep_ctf.z[0]
        );

        println!("\n=== WALL X[0] (exterior self-response) ===");
        println!("E+ X[0]:       {:.4}", ep_ctf.x[0]);
        println!(
            "Current X[0]:  {:.4} (ratio: {:.4})",
            our_ctf.x[0],
            our_ctf.x[0] / ep_ctf.x[0]
        );
        println!(
            "Proposed X[0]: {:.4} (ratio: {:.4})",
            proposed_ctf.x[0],
            proposed_ctf.x[0] / ep_ctf.x[0]
        );

        // Floor comparison
        println!("\n=== FLOOR CTF ===");
        let ep_floor = vec![
            ResolvedLayer::new_no_mass(25.175, 0.001),
            ResolvedLayer::new(1.13, 1400.0, 1000.0, 0.080),
        ];
        let ep_floor_ctf = calculate_ctf(&ep_floor, dt);
        let our_floor_ctf =
            calculate_ctf_simple(0.0396, 112000.0, dt, false, Some(1.13), Some(1400.0));

        println!("E+ Floor Z[0]:  {:.4}", ep_floor_ctf.z[0]);
        println!(
            "Our Floor Z[0]: {:.4} (ratio: {:.4})",
            our_floor_ctf.z[0],
            our_floor_ctf.z[0] / ep_floor_ctf.z[0]
        );
    }
}
