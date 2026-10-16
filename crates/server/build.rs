// Bakes the commit into the binary so /api/health can prove which build is
// running; deploy.sh refuses a deploy whose health `commit` doesn't match
// the tag it pulled. The Docker build passes GIT_SHA; local builds say
// "unknown".
fn main() {
    println!("cargo:rerun-if-env-changed=GIT_SHA");
    let sha = std::env::var("GIT_SHA")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=STOCKWATCHER_COMMIT={}", sha.trim());
}
