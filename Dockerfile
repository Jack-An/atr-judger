FROM rust:1.91.1-trixie AS build
RUN apt-get update \
    && apt-get install -y --no-install-recommends libseccomp-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --locked --release

FROM eclipse-temurin:21-jdk AS java

FROM node:22-trixie-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends g++ gcc libc6-dev libseccomp2 python3 tar \
    && rm -rf /var/lib/apt/lists/* \
    && npm install --prefix /usr/local --omit=dev --no-audit --no-fund \
        typescript@5.5.4 @types/node@22.19.1 \
    && npm cache clean --force
COPY --from=java /opt/java/openjdk /usr/lib/jvm/temurin-21
RUN ln -s /usr/lib/jvm/temurin-21/bin/java /usr/local/bin/java \
    && ln -s /usr/lib/jvm/temurin-21/bin/javac /usr/local/bin/javac
COPY --from=build /src/target/release/atr-judger /opt/atr-judger

ENV RUST_JUDGER_BIND=0.0.0.0:5060
ENV RUST_JUDGER_CGROUP_ROOT=/sys/fs/cgroup
EXPOSE 5060
ENTRYPOINT ["/opt/atr-judger"]
