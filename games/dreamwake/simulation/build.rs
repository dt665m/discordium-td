#[path = "../../../engine/build-support/source_identity.rs"]
mod source_identity;
fn main() {
    source_identity::generate("../../..");
}
