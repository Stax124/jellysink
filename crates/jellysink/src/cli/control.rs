//! `jellysink status`: one snapshot from a running daemon's `control.sock`.

use jellysink_core::VERSION;
use jellysink_core::config::Paths;
use jellysink_core::control::Client;

pub(crate) async fn cmd_status(paths: &Paths, json: bool) -> color_eyre::Result<()> {
    let (_, status) = Client::connect(paths, &format!("jellysink {VERSION} status")).await?;
    if json {
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
