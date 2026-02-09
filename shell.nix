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

    # For LLM CLI backends
    pkgs.nodejs_22
    pkgs.nodePackages.npm
  ];

  shellHook = ''
    export RUST_BACKTRACE=1
    export NPM_CONFIG_PREFIX=$HOME/.npm-global
    export PATH=$HOME/.local/bin:$NPM_CONFIG_PREFIX/bin:$PATH
    export LD_LIBRARY_PATH=${pkgs.openssl.out}/lib:$LD_LIBRARY_PATH

    echo ""
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
