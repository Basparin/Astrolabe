//! Run transfer entropy over caller-supplied named series.
//!
//! The assay estimators are library-public but reachable through no CLI/MCP
//! surface, whose tool schemas are code-bound. This example exposes the
//! substrate-agnostic path directly:
//!
//!     echo '{"seed":0,"series":[{"name":"a","values":[..]},..]}' \
//!       | cargo run -q -p astrolabe-assay --example series_te
//!
//! Prints the resulting CausalityCard as JSON on stdout.

use astrolabe_assay::diff::DiffConfig;
use astrolabe_assay::multivariate::transfer_entropy_bits;
use astrolabe_assay::rng::DeterministicRng;
use astrolabe_assay::transfer_entropy::{measure_transfer_entropy, NamedSeries};
use serde::Deserialize;
use std::io::Read;

#[derive(Deserialize)]
struct SeriesInput {
    name: String,
    values: Vec<i64>,
}

#[derive(Deserialize)]
struct Request {
    series: Vec<SeriesInput>,
    #[serde(default)]
    seed: u64,
    /// Override the registry default (200). Declared bounds are 2..=100_000
    /// (`ASSAY_{MIN,MAX}_TE_PERMUTATIONS`). The p-value floor is
    /// 1000/permutations permille, so 200 floors at 5 permille — too coarse to
    /// separate directions once n is large.
    #[serde(default)]
    permutations: Option<usize>,
    /// Shuffles used for the effective-TE null mean (default 200).
    #[serde(default)]
    ete_shuffles: Option<usize>,
    /// Also compute the Fisher-Yates null for comparison (doubles cost).
    #[serde(default)]
    also_shuffle_null: bool,
}

fn main() {
    let mut raw = String::new();
    std::io::stdin()
        .read_to_string(&mut raw)
        .expect("read stdin");
    let req: Request = serde_json::from_str(&raw).expect("parse request JSON");

    let series: Vec<NamedSeries> = req
        .series
        .into_iter()
        .map(|s| NamedSeries {
            name: s.name,
            values: s.values,
        })
        .collect();

    let mut cfg = DiffConfig::from_defaults().expect("registry-default DiffConfig");
    if let Some(p) = req.permutations {
        assert!(
            (2..=100_000).contains(&p),
            "permutations {p} outside declared bounds 2..=100000"
        );
        cfg.te_permutations = p;
    }
    eprintln!(
        "permutations={} p_floor={:.3} permille",
        cfg.te_permutations,
        1000.0 / cfg.te_permutations as f64
    );
    let card = match measure_transfer_entropy(&series, req.seed, &cfg) {
        Ok(card) => card,
        Err(e) => {
            eprintln!("{e:?}");
            std::process::exit(1);
        }
    };

    // Effective transfer entropy: observed TE minus the mean of the shuffled
    // null, using astrolabe's own estimator for both terms. Raw te_bits carry
    // finite-sample bias that grows with the state space, which is why they are
    // not comparable across directions; subtracting the null mean removes it.
    // The null MEAN converges far faster than a tail quantile, so a few hundred
    // shuffles suffice here where a p-value would need tens of thousands.
    let shuffles = req.ete_shuffles.unwrap_or(200);
    let mut out = Vec::new();
    for ev in &card.evaluations {
        let src = lookup(&series, &ev.from);
        let tgt = lookup(&series, &ev.to);
        let observed = transfer_entropy_bits(src, tgt, ev.best_lag);
        let mut rng = DeterministicRng::from_u64_labeled(req.seed, &format!("ete-{}", ev.from));
        let n = src.len();
        let mut sum = 0.0;
        let mut sum_shuf = 0.0;
        for _ in 0..shuffles {
            // Circular-shift surrogate: destroys the cross-relation to the
            // target while PRESERVING the source's own autocorrelation. A full
            // Fisher-Yates shuffle (what permutation_p_permille uses) destroys
            // both, which populates more joint cells and inflates the biased TE
            // estimate — so its null sits ABOVE the observed value for any
            // autocorrelated source, i.e. almost every real series.
            let shift = (rng.next_u64() % n as u64) as usize;
            let rotated: Vec<i64> = (0..n).map(|i| src[(i + shift) % n]).collect();
            sum += transfer_entropy_bits(&rotated, tgt, ev.best_lag);

            // The Fisher-Yates null is kept only for side-by-side comparison
            // and is off by default: its inflation is already established, and
            // computing it doubles the cost of every run.
            if req.also_shuffle_null {
                let mut shuffled = src.to_vec();
                for i in (1..n).rev() {
                    let j = (rng.next_u64() % (i as u64 + 1)) as usize;
                    shuffled.swap(i, j);
                }
                sum_shuf += transfer_entropy_bits(&shuffled, tgt, ev.best_lag);
            }
        }
        let null_mean = sum / shuffles as f64;
        let null_mean_shuffle = sum_shuf / shuffles as f64;
        out.push(serde_json::json!({
            "from": ev.from, "to": ev.to, "lag": ev.best_lag,
            "te_bits": observed,
            "null_circular": null_mean, "ete_circular": observed - null_mean,
            "null_shuffle": null_mean_shuffle, "ete_shuffle": observed - null_mean_shuffle,
            "p_value_permille": ev.p_value_permille,
        }));
    }
    println!(
        "{}",
        serde_json::json!({"trust": format!("{:?}", card.trust), "directions": out})
    );
}

fn lookup<'a>(series: &'a [NamedSeries], name: &str) -> &'a [i64] {
    &series
        .iter()
        .find(|s| s.name == name)
        .expect("series name from card must exist")
        .values
}
