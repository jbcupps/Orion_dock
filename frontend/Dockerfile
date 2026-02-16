# Orion frontend: build React app and serve with nginx. Proxy /api and /health to orion-api.
# Build from repo root: docker build -f frontend/Dockerfile -t orion-frontend:local .

# ---- build ----
FROM node:20-alpine AS builder

WORKDIR /app
COPY frontend/package.json frontend/package-lock.json* ./
RUN npm ci 2>/dev/null || npm install

COPY frontend/ ./
ENV VITE_API_URL=
RUN npm run build

# ---- runtime ----
FROM nginx:alpine

RUN apk add --no-cache curl

# nginx config template — ORION_API_TOKEN is substituted at container start
COPY frontend/nginx.conf.template /etc/nginx/nginx.conf.template
COPY frontend/docker-entrypoint.sh /docker-entrypoint.sh
RUN chmod +x /docker-entrypoint.sh
COPY --from=builder /app/dist /usr/share/nginx/html

EXPOSE 80
ENTRYPOINT ["/docker-entrypoint.sh"]
CMD ["nginx", "-g", "daemon off;"]
