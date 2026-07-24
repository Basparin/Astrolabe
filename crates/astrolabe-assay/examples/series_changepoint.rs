//! Run CUSUM change-point detection over a caller-supplied series.
//!
//! Companion to `series_te`: the changepoint estimator is library-public but
//! reachable through no CLI/MCP surface.
//!
//!     echo '{"name":"pop","values":[1.0,2.0,...],"seed":0}' \
//!       | cargo run -q -p astrolabe-assay --example series_changepoint

use astrolabe_assay::changepoint::measure_change_point;
use astrolabe_assay::diff::DiffConfig;
use serde::Deserialize;
use std::io::Read;

#[derive(Deserialize)]
struct Request {
    name: String,
    values: Vec<f64>,
    #[serde(default)]
    seed: u64,
}

fn main() {
    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw).expect("read stdin");
    let req: Request = serde_json::from_str(&raw).expect("parse request JSON");

    let cfg = DiffConfig::from_defaults().expect("registry-default DiffConfig");
    match measure_change_point(req.name, &req.values, req.seed, &cfg) {
        Ok(card) => println!("{}", serde_json::to_string(&card).expect("serialize card")),
        Err(e) => {
            eprintln!("{e:?}");
            std::process::exit(1);
        }
    }
}
