use serde_json::{Value, json};

pub(crate) fn capabilities() -> Value {
    json!({
        "PlayableMediaTypes": ["Video"],
        "SupportsMediaControl": true,
        "SupportsPersistentIdentifier": true,
        "SupportedCommands": [
            "ToggleFullscreen",
            "VolumeUp",
            "VolumeDown",
            "ToggleMute",
            "SetAudioStreamIndex",
            "SetSubtitleStreamIndex",
            "Mute",
            "Unmute",
            "SetVolume",
            "Play",
            "Playstate",
            "PlayNext",
            "PlayMediaSource"
        ]
    })
}

/// Permissive DirectPlay profile. Empty container/codec means "any".
/// TranscodingProfiles are present so the server is happy; we never play a
/// TranscodingUrl.
pub(crate) fn device_profile() -> Value {
    json!({
        "Name": "jellysink",
        "MaxStreamingBitrate": 1_200_000_000u64,
        "MaxStaticBitrate": 1_200_000_000u64,
        "MusicStreamingTranscodingBitrate": 1_280_000,
        "TimelineOffsetSeconds": 5,
        "DirectPlayProfiles": [
            {"Type": "Video"},
            {"Type": "Audio"}
        ],
        "TranscodingProfiles": [
            {"Type": "Audio"},
            {
                "Container": "ts",
                "Type": "Video",
                "Protocol": "hls",
                "AudioCodec": "aac,mp3,ac3,opus,flac,vorbis",
                "VideoCodec": "h264,h265,hevc,mpeg4,mpeg2video",
                "MaxAudioChannels": "8"
            }
        ],
        "ResponseProfiles": [],
        "ContainerProfiles": [],
        "CodecProfiles": [],
        "SubtitleProfiles": [
            {"Format": "srt", "Method": "External"},
            {"Format": "srt", "Method": "Embed"},
            {"Format": "ass", "Method": "External"},
            {"Format": "ass", "Method": "Embed"},
            {"Format": "sub", "Method": "Embed"},
            {"Format": "sub", "Method": "External"},
            {"Format": "ssa", "Method": "Embed"},
            {"Format": "ssa", "Method": "External"},
            {"Format": "smi", "Method": "Embed"},
            {"Format": "smi", "Method": "External"},
            {"Format": "pgssub", "Method": "Embed"},
            {"Format": "dvdsub", "Method": "Embed"},
            {"Format": "dvbsub", "Method": "Embed"},
            {"Format": "pgs", "Method": "Embed"}
        ]
    })
}

#[cfg(test)]
mod tests {
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
}
