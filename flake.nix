{
  description = "rain-org-health — org modernization-debt scanner and dashboard.";

  inputs = {
    rainix.url = "github:rainlanguage/rainix";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      flake-utils,
      rainix,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = rainix.pkgs.${system};
        # The org-health scanner: pure signal detection (unit + mutation tested,
        # run in-build via doCheck) plus gh/curl orchestration. Invoked directly
        # as `nix run .#roh-scan` — no bash wrapper.
        #
        # The crate is a workspace member (plugins/rain-org-health-check/roh-scan)
        # but the workspace Cargo.toml + Cargo.lock live at the repo root, so the
        # build src must be rooted there (not the crate dir) or cargo can't resolve
        # the lockfile. A fileset keeps the derivation from rebuilding on unrelated
        # repo changes (docs, dashboard, workflows).
        roh-scan = pkgs.rustPlatform.buildRustPackage {
          pname = "roh-scan";
          version = "0.1.0";
          src = pkgs.lib.fileset.toSource {
            root = ./.;
            fileset = pkgs.lib.fileset.unions [
              ./Cargo.toml
              ./Cargo.lock
              ./plugins/rain-org-health-check/roh-scan
            ];
          };
          cargoLock.lockFile = ./Cargo.lock;
        };
        # Reproducible headless render of the dashboard, so an eyeball on the
        # deployed page (or a CI visual check) is one pinned command rather than
        # an ad-hoc chromium incantation: `nix run .#screenshot -- [site] [out]`.
        # The page fetches health.json at runtime, so a `file://` open can't work
        # — we serve site/ over a local HTTP server and point headless chromium at
        # it. A bundled fontconfig (dejavu) is REQUIRED: without fonts, headless
        # chromium lays the page out but draws every text label blank.
        screenshot = pkgs.writeShellApplication {
          name = "screenshot";
          runtimeInputs = [
            pkgs.chromium
            pkgs.python3
            pkgs.curl
          ];
          text = ''
            site="''${1:-site}"
            out="''${2:-dashboard.png}"
            port="''${PORT:-8799}"
            export FONTCONFIG_FILE=${pkgs.makeFontsConf { fontDirectories = [ pkgs.dejavu_fonts ]; }}
            python3 -m http.server "$port" --directory "$site" >/dev/null 2>&1 &
            srv=$!
            trap 'kill "$srv" 2>/dev/null || true' EXIT
            # Wait for the server to accept connections; fail fast if it never comes up
            # (a fixed sleep hides startup failures and is brittle on a slow host/CI).
            ready=
            for _ in $(seq 1 50); do
              if curl -fsS -o /dev/null "http://127.0.0.1:$port/"; then
                ready=1
                break
              fi
              sleep 0.1
            done
            [ -n "$ready" ] || {
              echo "screenshot: local server never came up on :$port" >&2
              exit 1
            }
            # --disable-dev-shm-usage: containers/CI often have a tiny /dev/shm, which
            # crashes headless chromium; write shared memory to /tmp instead.
            chromium \
              --headless --no-sandbox --disable-gpu --disable-dev-shm-usage --hide-scrollbars \
              --user-data-dir="$(mktemp -d)" \
              --force-color-profile=srgb \
              --window-size="''${WIDTH:-1300},''${HEIGHT:-4200}" \
              --virtual-time-budget=9000 \
              --screenshot="$out" \
              "http://127.0.0.1:$port/index.html"
            echo "wrote $out ($(wc -c <"$out") bytes)"
          '';
        };
      in
      {
        packages = {
          inherit roh-scan screenshot;
          default = roh-scan;
        };
        # `nix develop` composes rainix's default devshell, which wires the same
        # pre-commit hooks CI runs (prettier-rainix, cargo fmt/clippy, deadnix,
        # nixfmt, statix, taplo, yamlfmt, shellcheck) with rainix's pinned tool
        # versions — so `pre-commit run --all-files` reproduces `rs-static` exactly.
        # Plus roh-scan's runtime deps (gh authed by the caller, curl).
        devShells.default = pkgs.mkShell {
          inherit (rainix.devShells.${system}.default) shellHook;
          packages = [
            roh-scan
            pkgs.gh
            pkgs.curl
          ];
          inputsFrom = [ rainix.devShells.${system}.default ];
        };
      }
    );
}
