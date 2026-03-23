#----- Build Stage -----

FROM debian:trixie-slim AS builder

ENV DEBIAN_FRONTEND=noninteractive

WORKDIR /app

# install dependencies
RUN apt-get update && apt-get install -y \
    curl \
    git \
    build-essential \
    nodejs \
    npm \
    && rm -rf /var/lib/apt/lists/*

# install rust
RUN curl https://sh.rustup.rs -sSf | sh -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"

COPY . .

# build frontend
WORKDIR /app/web
RUN npm install && npm run build

# build backend
WORKDIR /app
RUN cargo build --release

#----- Runtime Stage -----
FROM debian:trixie-slim

WORKDIR /app

ENV RESIDUUM_GATEWAY_BIND=0.0.0.0

COPY --from=builder /app/target/release/residuum /app/residuum

RUN chmod +x residuum

RUN useradd -m residuum

USER residuum

VOLUME [ "/home/residuum/.residuum" ]

EXPOSE 7700

CMD ["./residuum", "serve", "--foreground", "--debug"]