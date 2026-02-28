.PHONY: all
MAKEFLAGS += -j

caddy:
	@caddy run

tailwindwatch:
	@tailwindcss -i ./src/styles.css -o ./static/styles.css --watch
backdev:
	@cargo watch -x "run --bin weather"
dev: tailwindwatch backdev caddy
	

tailwindcompile:
	@tailwindcss -i ./src/styles.css -o ./static/styles.css --minify
backpre: tailwindcompile
	@cargo build --release --bin weather && ./target/release/weather
pre: backpre caddy
