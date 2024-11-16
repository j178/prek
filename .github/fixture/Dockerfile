FROM rust:latest

ENV RUST_BACKTRACE=1
WORKDIR /app

COPY --from=docker:latest /usr/local/bin/docker  /usr/local/bin/

COPY . .

ENTRYPOINT ["cargo", "test", "--package", "pre-commit-rs", "languages::docker::tests::test_get_docker_path", "--", "--ignored", "--exact", "--show-output"]
