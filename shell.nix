let
  pkgs = import <nixpkgs> {};
in
pkgs.mkShell {
  nativeBuildInputs = with pkgs; [
    rustup
    caddy
    tailwindcss_4
  ];
}
