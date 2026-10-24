// Rust guideline compliant 2026-02-21
fn main() {
    let powers = [1.0_f32,1.0,20.0,40.0,80.0,60.0,20.0,4.0,1.0];
    let noises = [1.0_f32,1.0,1.0,1.0,4.0,4.0,4.0,4.0,4.0];
    let mut histories = Vec::new();
    for reduction in [2.0_f32,10.0,100.0] {
        let mut previous_gain = 1.0_f32;
        let mut previous_power = 0.0_f32;
        let mut priors = Vec::new();
        for (&power,&noise) in powers.iter().zip(&noises) {
            // Frozen production recurrence: user-controlled final gain feeds
            // back into the source SNR estimator on the next frame.
            let prior = 0.98*previous_gain*previous_gain*previous_power/noise
                +0.02*(power/noise-1.0).max(0.0);
            previous_gain = (prior/(prior+reduction)).max(0.01);
            previous_power = power;
            priors.push(prior);
        }
        histories.push(priors);
    }
    let mut maximum_spread = 0.0_f32;
    for frame in 0..powers.len() {
        let minimum = histories.iter().map(|h|h[frame]).fold(f32::INFINITY,f32::min);
        let maximum = histories.iter().map(|h|h[frame]).fold(0.0,f32::max);
        maximum_spread = maximum_spread.max(maximum-minimum);
    }
    println!("{{\"controls\":3,\"frames\":9,\"maximum_prior_spread\":{maximum_spread},\"histories\":{histories:?}}}");
    assert!(maximum_spread<3e-6,"user attenuation must not alter the inferred source SNR");
}
