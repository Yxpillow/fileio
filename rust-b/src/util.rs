use std::path::Path;
use std::fs;

pub fn ensure_dir(path: &Path) -> anyhow::Result<()> {
    if !path.exists() {
        fs::create_dir_all(path)?;
    }
    Ok(())
}

pub fn format_time(t: Option<std::time::SystemTime>) -> String {
    match t.and_then(|x| x.duration_since(std::time::UNIX_EPOCH).ok()) {
        Some(d) => format!("{}", d.as_secs()),
        None => "0".into(),
    }
}

pub fn rand_u32() -> u32 {
    use rand::RngCore;
    let mut rng = rand::rngs::OsRng;
    rng.next_u32()
}