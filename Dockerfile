FROM busybox:1.36.1 as rename
WORKDIR /app
COPY target/aarch64-unknown-linux-musl/release/kapplier kapplier-arm64
COPY target/x86_64-unknown-linux-musl/release/kapplier kapplier-amd64

FROM gcr.io/distroless/base-nossl-debian12:nonroot
ARG TARGETARCH
COPY --from=rename /app/kapplier-$TARGETARCH /app/kapplier
ENTRYPOINT [ "/app/kapplier" ]
