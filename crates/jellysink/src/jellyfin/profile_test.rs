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
