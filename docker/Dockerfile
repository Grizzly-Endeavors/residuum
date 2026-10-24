#----- Web Build Stage -----
# Node 24 is Active LTS; satisfies web/package.json's engines.node
# (^22.13.0 || ^24.0.0 || >=26.0.0) which apt's Node 20 on trixie does not.
FROM node:24-bookworm-slim AS web-builder

WORKDIR /app/web

COPY web/ .
RUN npm install && npm run build

#----- Build Stage -----

FROM debian:trixie-slim AS builder

ENV DEBIAN_FRONTEND=noninteractive

WORKDIR /app

# install dependencies
RUN apt-get update && apt-get install -y \
    curl \
    git \
    build-essential \
    && rm -rf /var/lib/apt/lists/*

# install rust
RUN curl https://sh.rustup.rs -sSf | sh -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"

COPY . .
COPY --from=web-builder /app/web/dist /app/web/dist

# The version and commit the binary reports (hub status, the update check,
# bug reports). The context leaves out `.git`, so build.rs can't read them from
# git here; `just docker-build` and docker-compose.yml pass them from the
# checkout. Without them the version is `dev` and the commit unknown.
# Declared just before the build so a new commit keeps the layers above cached.
ARG RESIDUUM_VERSION=""
ARG RESIDUUM_GIT_COMMIT=""

# build backend
RUN cargo build --release

#----- Runtime Stage -----
FROM debian:trixie-slim

WORKDIR /app

ENV RESIDUUM_GATEWAY_BIND=0.0.0.0

# ca-certificates: rustls-platform-verifier reads the system trust store, so
# HTTPS to model providers fails without it. git: the agent shells out and
# needs a real runtime present in the image (the [tools] PATH only covers
# single-file static binaries).
# Kept ahead of the COPY so this layer stays cached across releases.
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates git && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/residuum /app/residuum

RUN chmod +x residuum

RUN useradd -m residuum

USER residuum

# A volume mounted where the image has no directory starts out owned by root,
# which the residuum user can't write to; one the image has takes its owner.
RUN mkdir -p /home/residuum/.residuum

VOLUME [ "/home/residuum/.residuum" ]

EXPOSE 7700

CMD ["./residuum", "serve", "--foreground"]