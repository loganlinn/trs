{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = {
    self,
    nixpkgs,
    flake-utils,
  }:
    {
      overlay = final: prev: {
        "${self.name}" = prev.callPackage ./default.nix {};
      };
    }
    // flake-utils.lib.eachDefaultSystem (
      system: let
        pkgs = import nixpkgs {inherit system;};
        this = pkgs.callPackage ./default.nix {inherit (self) name;};
      in {
        packages = {
          "${self.name}" = this;
          default = this;
        };

        devShells.default = pkgs.mkShell {
          name = "${self.name}-develop";
          inputsFrom = [this];
        };
      }
    );
}
