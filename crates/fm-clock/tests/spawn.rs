//! A real detached `fire`: the probe binary stands in for `fm`, and must outlive nothing but write
//! its arguments.

use std::time::{Duration, Instant};

use fm_clock::{DetachedSpawn, StartRun};
use fm_model::Trigger;

#[test]
fn a_detached_fire_starts_with_the_contract_arguments() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("state");
    std::fs::create_dir_all(&home).unwrap();
    let mut spawn =
        DetachedSpawn::new(env!("CARGO_BIN_EXE_fm-clock-probe")).with_log_dir(home.join("logs"));
    spawn.env = vec![
        ("FM_HOME".into(), home.to_string_lossy().into_owned()),
        ("FM_PROBE_FIRE_DELAY_MS".into(), "0".into()),
    ];
    spawn
        .start(
            "user:/r:a",
            Trigger::Schedule,
            Some("2026-09-17T07:00:00.000Z".into()),
            true,
        )
        .unwrap();
    let fired = home.join("fired.txt");
    let deadline = Instant::now() + Duration::from_secs(30);
    while !fired.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    std::thread::sleep(Duration::from_millis(200));
    let text = std::fs::read_to_string(&fired).expect("the detached fire never ran");
    assert_eq!(
        text.trim(),
        "fire --key user:/r:a --trigger schedule --scheduled-for 2026-09-17T07:00:00.000Z --late"
    );
}
