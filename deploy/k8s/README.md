# Kubernetes Scaffold

Minimal manifests for running Orion's full stack in Kubernetes. **Matches the Docker Compose full-stack experience**: postgres, ollama, orion-api, frontend. Not the primary deployment path; Docker Compose is. Use this as a starting point for production or cluster rollout.

## Contract

| Resource         | Purpose |
|------------------|---------|
| `namespace.yaml` | Optional `orion` namespace |
| `configmap.yaml` | `MEMORY_BACKEND`, `BIRTH_MODEL`, `LOCAL_LLM_BASE_URL`, `ORION_DATA_DIR`, `PRO_MODE_SIDECAR_URL` |
| `secret.yaml`    | `DATABASE_URL` (required), optional `OPENAI_API_KEY` |
| `postgres.yaml`  | Postgres + pgvector, PVC, Secret for DB password |
| `ollama.yaml`    | Ollama service and PVC for models |
| `orion-api.yaml` | Orion HTTP API (health, status, identities); proxies to Ollama |
| `frontend.yaml`  | Web UI (React); proxies /api, /health to orion-api |

## Order and Prerequisites

1. Create namespace: `kubectl apply -f namespace.yaml`
2. Set secrets: in `postgres.yaml` and `secret.yaml`, set Postgres password and `DATABASE_URL=postgres://orion:<password>@postgres:5432/orion`.
3. Apply in order:
   ```bash
   kubectl apply -f postgres.yaml
   kubectl apply -f ollama.yaml
   kubectl apply -f configmap.yaml -f secret.yaml
   kubectl apply -f orion-api.yaml
   kubectl apply -f frontend.yaml
   ```

## Building the full-stack images

From repo root:

```bash
# Default: orion-api:latest, orion-frontend:latest
./scripts/build-full-stack-image.sh

# With registry prefix (e.g. for push)
./scripts/build-full-stack-image.sh myreg.io/
```

Windows: `.\scripts\build-full-stack-image.ps1 [prefix]`

Or with Docker directly:
```bash
docker build -f docker/Dockerfile.api -t orion-api:latest .
docker build -f frontend/Dockerfile -t orion-frontend:latest .
```

## Accessing the Web UI

- **LoadBalancer:** Frontend Service uses `type: LoadBalancer`. After apply, `kubectl get svc frontend -n orion` for the external IP. Open `http://<external-ip>` for the web UI.
- **NodePort / port-forward:** For local clusters (kind, minikube), change the frontend Service to `type: NodePort` or use `kubectl port-forward svc/frontend 3000:80 -n orion` and open http://localhost:3000.

## Notes

- **Images:** Ensure `orion-api:latest` and `orion-frontend:latest` are available in the cluster (e.g. `kind load docker-image orion-api:latest`, or push to a registry and set `imagePullPolicy`).
- **Migrations:** Postgres schema (memories, birth, embeddings, edges) is applied on first connection from orion-api.
- **Birth model:** After Ollama is running, pull the birth model:  
  `kubectl exec -it deploy/ollama -n orion -- ollama pull qwen2.5:3b-instruct`
- **Secrets:** Prefer a secret manager (External Secrets, Sealed Secrets) instead of plain `secret.yaml`; override `DATABASE_URL` and Postgres password there.
- **Pro sidecar (optional):** To enable Pro-tier best-of-two provider comparison, deploy the `services/pro-router/` Python service as a separate pod and set `PRO_MODE_SIDECAR_URL` in the configmap to its service URL (e.g. `http://pro-router:8100`). A Kubernetes manifest for the sidecar is not yet provided; create a Deployment + Service from `services/pro-router/Dockerfile`.
