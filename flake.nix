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
      in
      {
        packages.roh-scan = roh-scan;
        packages.default = roh-scan;
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
