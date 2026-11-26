# Rust dev image (Debian trixie) + LLVM/MLIR 23 from apt.llvm.org, for melior.
FROM mcr.microsoft.com/devcontainers/rust:1-trixie

ARG LLVM_VERSION=23

# Pin the apt.llvm.org repo to the llvm-toolchain-trixie-${LLVM_VERSION} branch so
# upgrades never move off the requested major version.
RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends ca-certificates curl gnupg; \
    curl -fsSL https://apt.llvm.org/llvm-snapshot.gpg.key \
      | gpg --dearmor -o /usr/share/keyrings/llvm-archive-keyring.gpg; \
    echo "deb [signed-by=/usr/share/keyrings/llvm-archive-keyring.gpg] http://apt.llvm.org/trixie/ llvm-toolchain-trixie-${LLVM_VERSION} main" \
      > /etc/apt/sources.list.d/llvm.list; \
    apt-get update; \
    apt-get install -y --no-install-recommends \
      llvm-${LLVM_VERSION} llvm-${LLVM_VERSION}-dev llvm-${LLVM_VERSION}-tools \
      libmlir-${LLVM_VERSION}-dev mlir-${LLVM_VERSION}-tools \
      clang-${LLVM_VERSION} libclang-${LLVM_VERSION}-dev libpolly-${LLVM_VERSION}-dev \
      lld-${LLVM_VERSION} clangd-${LLVM_VERSION} \
      zlib1g-dev libzstd-dev libxml2-dev libffi-dev libedit-dev libncurses-dev; \
    rm -rf /var/lib/apt/lists/*

# Graphviz for rendering DAGs (`dot -Tsvg in.dot -o out.svg`). Kept in its own
# layer so the LLVM layer above stays cached. Fonts are needed for node labels
# since --no-install-recommends skips Graphviz's default font packages.
RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends graphviz fonts-dejavu-core; \
    rm -rf /var/lib/apt/lists/*

# Put LLVM tools (mlir-opt, mlir-translate, llvm-config, clang, ...) on PATH.
ENV LLVM_VERSION=${LLVM_VERSION} \
    LLVM_PREFIX=/usr/lib/llvm-${LLVM_VERSION}
ENV PATH=${LLVM_PREFIX}/bin:${PATH} \
    # Consumed by mlir-sys 230.x and tblgen 0.10.x (melior 0.30.x)
    MLIR_SYS_230_PREFIX=${LLVM_PREFIX} \
    TABLEGEN_230_PREFIX=${LLVM_PREFIX} \
    LLVM_SYS_230_PREFIX=${LLVM_PREFIX} \
    LIBCLANG_PATH=${LLVM_PREFIX}/lib \
    LD_LIBRARY_PATH=${LLVM_PREFIX}/lib
