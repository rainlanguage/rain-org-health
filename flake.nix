{
  description = "rain-org-health — org modernization-debt scanner and dashboard.";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    { nixpkgs, flake-utils, ... }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs { inherit system; };
        # The org-health scanner: pure signal detection (unit + mutation tested,
        # run in-build via doCheck) plus gh/curl orchestration. Invoked directly
        # as `nix run .#roh-scan` — no bash wrapper.
        roh-scan = pkgs.rustPlatform.buildRustPackage {
          pname = "roh-scan";
          version = "0.1.0";
          src = ./plugins/rain-org-health-check/roh-scan;
          cargoLock.lockFile = ./Cargo.lock;
        };
      in
      {
        packages.roh-scan = roh-scan;
        packages.default = roh-scan;
        # Runtime deps roh-scan shells out to (gh authed by the caller).
        devShells.default = pkgs.mkShell {
          packages = [
            roh-scan
            pkgs.gh
            pkgs.curl
            pkgs.coreutils
          ];
        };
      }
    );
}
