use super::*;

#[test]
fn concurrent_writers_to_one_path_leave_a_whole_file_rather_than_a_mix() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cover.img");
    let payloads = [vec![b'a'; 64 * 1024], vec![b'b'; 32 * 1024]];

    std::thread::scope(|scope| {
        for payload in &payloads {
            scope.spawn(|| atomic_write(&path, payload, 0o644).unwrap());
        }
    });

    let written = fs::read(&path).unwrap();
    assert!(
        payloads.contains(&written),
        "{} bytes is neither payload",
        written.len()
    );
    assert_eq!(
        fs::read_dir(dir.path()).unwrap().count(),
        1,
        "a tmp file was left behind"
    );
}
