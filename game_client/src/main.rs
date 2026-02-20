mod plugins;

use bevy::prelude::*;
use clap::Parser;
use plugins::prelude::*;

fn main() {
    let args = ClientArgs::parse();

    App::new()
        .insert_resource(args)
        .add_plugins(GameClientPlugin)
        .run();
}
