# ADR 0002: Zero-Downtime Blue/Green Updates with Docker Compose

- Status: Accepted
- Date: 2026-08-14

## Decision

Kubernetes is not part of the current deployment target. Docker Compose runs
`api-blue` and `api-green` as independently replaceable services, with NGINX
providing a stable entry point. Only one API color is active at a time. A
release starts the inactive color, waits for its `/health/ready` check, writes
the active upstream, validates NGINX, and performs a graceful NGINX reload.
Only after the reload succeeds does it stop the old color.

SIGTERM marks the instance as draining, so `/health/ready` fails, waits five
seconds for existing gateway connections to drain, closes the listener, and
finishes in-flight requests. The deployment has already switched NGINX away
from that color before sending SIGTERM. Compose provides a 45-second
`stop_grace_period`.

Background polling does not run in either API replica. A separately restartable `worker` service owns durable queue consumers. One Worker is the default; multiple Workers remain safe because claims and leases are coordinated through PostgreSQL.

## Limitations

Docker Compose is not a distributed scheduler. This design prevents application-process downtime during updates on one host, but it cannot tolerate failure of the host, Docker daemon, single NGINX instance, or host network. Cross-host availability will require an orchestrator or an external load balancer.

Database migrations must follow expand/migrate/contract so adjacent
application versions can run concurrently. WebSockets, long polling, and
unusually long requests must finish within the grace period. The active color
is kept in the deployment-local ignored `.active-api` file, and the generated
upstream fragment is kept in the ignored
`nginx/conf.d/active-upstream.conf` file. Losing those files requires an
operator to verify which color is serving before restarting the gateway.
