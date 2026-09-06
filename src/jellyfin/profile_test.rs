use super::*;

#[test]
fn direct_play_profiles_have_no_codec_list() {
    let p = device_profile();
    for entry in p["DirectPlayProfiles"].as_array().unwrap() {
        assert!(entry.get("Container").is_none());
        assert!(entry.get("VideoCodec").is_none());
        assert!(entry.get("AudioCodec").is_none());
    }
}

#[test]
fn bitrate_cap_is_high() {
    let p = device_profile();
    assert_eq!(p["MaxStreamingBitrate"], 1_200_000_000u64);
}
