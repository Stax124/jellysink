use super::resume_seek_ticks;

#[test]
fn resume_seek_ticks_applies_only_positive_offsets() {
    assert_eq!(resume_seek_ticks(None), None);
    assert_eq!(resume_seek_ticks(Some(0)), None);
    assert_eq!(resume_seek_ticks(Some(-1)), None);
    assert_eq!(resume_seek_ticks(Some(1)), Some(1));
    assert_eq!(resume_seek_ticks(Some(600_000_000)), Some(600_000_000));
}
