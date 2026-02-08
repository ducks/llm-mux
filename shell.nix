{ pkgs ? import <nixpkgs> {} }:

pkgs.mkShell {
  buildInputs = with pkgs; [
    rustc
    cargo
    rust-analyzer
    clippy
    rustfmt
    pkg-config
    openssl
  ];

  shellHook = ''
    echo "llmux dev shell"
    echo "  rustc: $(rustc --version)"
    echo "  cargo: $(cargo --version)"
    echo ""
    echo "Commands:"
    echo "  cargo build    - Build the project"
    echo "  cargo test     - Run tests"
    echo "  cargo clippy   - Lint"
  '';
}
