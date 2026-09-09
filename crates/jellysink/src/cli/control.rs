//! The one-shot commands that talk to an already-running daemon over
//! `stop.sock`.

use jellysink_core::config::Paths;
use jellysink_core::instance;

pub(crate) fn cmd_stop(paths: &Paths) -> color_eyre::Result<()> {
    instance::request_stop(paths)
}

pub(crate) fn cmd_status(paths: &Paths, json: bool) -> color_eyre::Result<()> {
    let status = instance::request_status(paths)?;
    if json {
        let mut status = status;
        if let Some(now_playing) = &mut status.now_playing {
            now_playing.art_url =
                jellysink_core::jellyfin::url::redact_api_key(&now_playing.art_url);
        }
        println!("{}", serde_json::to_string_pretty(&status)?);
        return Ok(());
    }
    println!("server:   {}", status.server);
    println!("user:     {}", status.username);
    match status.now_playing {
        Some(now_playing) => {
            println!(
                "playing:  {} (paused: {}, muted: {}, volume: {})",
                now_playing.title,
                if now_playing.is_paused { "yes" } else { "no" },
                if now_playing.is_muted { "yes" } else { "no" },
                now_playing.volume
            );
            println!(
                "position: {}",
                jellysink_core::ticks::format_hms(now_playing.position_ticks)
            );
            println!(
                "queue:    {}/{}",
                now_playing.queue_index + 1,
                now_playing.queue_len
            );
        }
        None => println!("playing:  nothing"),
    }
    Ok(())
}
