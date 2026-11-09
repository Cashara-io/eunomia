FROM oven/bun:1 AS build
WORKDIR /app
COPY package.json bun.lock ./
RUN bun install --frozen-lockfile
COPY . .
# Browser calls same-origin /api/*, proxied to this compose service name
# (see next.config.ts) -- so one prebuilt image works for every host/IP.
ENV BACKEND_INTERNAL_URL=http://backend:8001
# Release tag, passed by .github/workflows/release.yml; shown in the sidebar.
ARG APP_VERSION=dev
ENV NEXT_PUBLIC_APP_VERSION=$APP_VERSION
RUN bun run build

FROM oven/bun:1
# Apply the available apt and zlib security patches in the runtime base.
RUN apt-get update && apt-get install -y --only-upgrade --no-install-recommends apt libapt-pkg7.0 zlib1g && rm -rf /var/lib/apt/lists/*
WORKDIR /app
ENV BACKEND_INTERNAL_URL=http://backend:8001
COPY --from=build /app ./
EXPOSE 3000
CMD ["bun", "run", "start"]
