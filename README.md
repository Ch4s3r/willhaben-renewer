# willhaben-renewer

> Re-publishes expired willhaben.at adverts using Rust + `thirtyfour` (chromedriver).

### Run via Nix directly from Git (git+ssh)
Requires ssh access (e.g. you have a deploy key or your GitHub key loaded in your agent):
```bash
export WILLHABEN_USERNAME="your@login"
export WILLHABEN_PASSWORD="secret"
nix run github:Ch4s3r/willhaben-renewer
```

You can omit the trailing attribute (`#...`) because the flake's default package is the wrapped binary (chromedriver on PATH). Pass flags after `--`.

## Run
```bash
export WILLHABEN_USERNAME="your@login"
export WILLHABEN_PASSWORD="secret"
cargo run --release
# or
cargo run -- --username "$WILLHABEN_USERNAME" --password "$WILLHABEN_PASSWORD" --headless true
```

If SMS 2FA appears you'll be prompted for the 4‑digit code (reference printed if present).

## Flow
1. Start / (auto) spawn chromedriver.
2. Open homepage → accept cookies ("Cookies akzeptieren").
3. Click login → fill creds → submit.
4. If OTP: show reference, prompt code, fill, submit.
5. Poll until login links gone.
6. Open expired adverts page.
7. For each "Neu veröffentlichen": Weiter → Weiter → Veröffentlichen.
8. Loop until none left → quit & kill chromedriver.
