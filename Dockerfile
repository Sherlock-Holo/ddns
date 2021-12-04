FROM clux/muslrust:stable AS builder

COPY . /volume

WORKDIR /volume

RUN cargo build --release

FROM alpine:latest

RUN apk --no-cache add ca-certificates

COPY --from=builder /volume/target/x86_64-unknown-linux-musl/release/ddns /ddns

CMD /ddns
