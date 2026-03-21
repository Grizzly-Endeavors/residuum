FROM debian:trixie-slim

ENV DEBIAN_FRONTEND=noninteractive
ENV TZ=Etc/UTC

# 1.working directory
WORKDIR /app

# 2.install required tools
RUN apt-get update && apt-get install -y curl socat && rm -rf /var/lib/apt/lists/*

# 3.download binary
RUN curl -L -o residuum https://github.com/Grizzly-Endeavors/residuum/releases/download/v2026.03.14/residuum-linux-x86_64

# 4.make executable
RUN chmod +x residuum

# 5.create user
RUN useradd -m residuum

# 6.create config dirrectory and set permissions
RUN mkdir -p /home/residuum/.residuum \ && chown -R residuum:residuum /home/residuum

# 7.switch to the created user
USER residuum

# 8.volume for persistence
VOLUME [ "/home/residuum/.residuum" ]

# port for web interface and API
EXPOSE 7700

# 9.run app
CMD ["./residuum", "serve", "--foreground"]