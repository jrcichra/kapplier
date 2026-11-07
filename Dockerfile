FROM gcr.io/distroless/cc-debian13:nonroot
ARG TARGETARCH
COPY docker-bin/kapplier-${TARGETARCH} /app/kapplier
ENTRYPOINT ["/app/kapplier"]
