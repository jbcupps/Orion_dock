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

# Default nginx.conf serves static and proxies /api, /health, /ready to orion-api
COPY frontend/nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=builder /app/dist /usr/share/nginx/html

EXPOSE 80
CMD ["nginx", "-g", "daemon off;"]
