mod plugins;

use bevy::prelude::*;
#[cfg(not(target_arch = "wasm32"))]
use clap::Parser;
use plugins::prelude::*;

fn main() {
    #[cfg(not(target_arch = "wasm32"))]
    let args = ClientArgs::parse();
    #[cfg(target_arch = "wasm32")]
    let args = ClientArgs::default();

    App::new()
        .insert_resource(args)
        .add_plugins(GameClientPlugin)
        .run();
}
