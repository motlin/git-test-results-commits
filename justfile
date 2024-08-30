default:
    cargo fmt
    cargo build --release
    cp ./target/release/test-results ~/bin
