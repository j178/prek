lint:
	cargo fmt
	cargo clippy --all-targets --all-features --workspace -- -D warnings

test-unit:
	cargo insta test --review --bin prefligit -- $F

test-all-unit:
	cargo insta test --review --workspace --lib --bins

test-integration:
	cargo insta test --review --test $T -- $F

test-all-integration:
	cargo insta test --review --test '*'

test:
	cargo test --all-targets --all-features --workspace
