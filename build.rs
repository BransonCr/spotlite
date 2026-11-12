//! `builtin_player`: this build has spotlite's own player (`spotlite daemon`). Always on
//! Windows (there is no spotifyd there); on other systems with `--features player`.
fn main() {
    println!("cargo::rustc-check-cfg=cfg(builtin_player)");
    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    if windows || std::env::var_os("CARGO_FEATURE_PLAYER").is_some() {
        println!("cargo::rustc-cfg=builtin_player");
    }
}
