# secrets-helper: the credential broker's sidecar (docs/architecture/specs/broker.md). Small, stateless,
# non-root; docker-compose.yml runs it on a read-only root filesystem with no published port.
FROM node:24.22.2-alpine3.24 AS build
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY tsconfig.json ./
COPY src ./src
COPY test ./test
# the unit tests run in the build, so an image that builds has passed them
RUN npx tsc -p tsconfig.json && node --test dist/test/*.test.js && npm prune --omit=dev --no-audit --no-fund

FROM node:24.22.2-alpine3.24
WORKDIR /app
ENV NODE_ENV=production
COPY --from=build /app/package.json ./package.json
COPY --from=build /app/node_modules ./node_modules
COPY --from=build /app/dist/src ./dist/src
USER node
EXPOSE 8080
CMD ["node", "dist/src/main.js"]
