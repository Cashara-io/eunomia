# Tiny backup image. The surrealdb image is distroless and has no shell, so we
# copy only its `surreal` binary (dynamic glibc, hence debian-slim, not alpine)
# and add openssl for encryption. Nothing is downloaded at runtime.
FROM surrealdb/surrealdb:v3.3.5 AS surreal

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends openssl tzdata ca-certificates \
 && rm -rf /var/lib/apt/lists/*
COPY --from=surreal /surreal /usr/local/bin/surreal
COPY eunomia-backup.sh /usr/local/bin/eunomia-backup
ENTRYPOINT ["/usr/local/bin/eunomia-backup"]
CMD ["loop"]
