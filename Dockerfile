FROM ekidd/rust-musl-builder:latest AS builder

COPY . /home/rust/src

WORKDIR /home/rust/src

RUN cargo build --release

FROM alpine:latest

RUN apk --no-cache add ca-certificates

COPY --from=builder /home/rust/src/target/x86_64-unknown-linux-musl/release/ddns /ddns

CMD /ddns
