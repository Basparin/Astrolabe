//! Conditional mutual information over caller-supplied discrete series.
//!
//! Answers "does Y tell us anything about X that Z does not already?" —
//! `I(X;Y|Z)` in bits. Companion to `series_te` and `series_changepoint`:
//! library-public, no tool surface.
//!
//!     echo '{"x":[..],"y":[..],"z":[..]}' \
//!       | cargo run -q -p astrolabe-assay --example series_cmi

use astrolabe_assay::multivariate::conditional_mi_bits;
use serde::Deserialize;
use std::io::Read;

#[derive(Deserialize)]
struct Request {
    x: Vec<i64>,
    y: Vec<i64>,
    z: Vec<i64>,
}

fn main() {
    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw).expect("read stdin");
    let req: Request = serde_json::from_str(&raw).expect("parse request JSON");
    let n = req.x.len().min(req.y.len()).min(req.z.len());
    let bits = conditional_mi_bits(&req.x[..n], &req.y[..n], &req.z[..n]);
    println!("{}", serde_json::json!({"cmi_bits": bits, "n": n}));
}
