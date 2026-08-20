# ADR 0002: Zero-Downtime Blue/Green Updates with Docker Compose

- Status: Accepted
- Date: 2026-08-14

## Decision

Kubernetes is not part of the current deployment target. Docker Compose runs `api-blue` and `api-green` concurrently, with NGINX providing a stable entry point while Compose health checks gate rollout. A release replaces blue and green sequentially, waiting for each new instance to become ready before replacing the other.

SIGTERM starts this sequence: mark the instance as draining, fail readiness, wait five seconds for the gateway to stop selecting it, close the listener, and finish in-flight requests. Compose provides a 45-second `stop_grace_period`.

Background polling does not run in either API replica. A separately restartable `worker` service owns durable queue consumers. One Worker is the default; multiple Workers remain safe because claims and leases are coordinated through PostgreSQL.

## Limitations

Docker Compose is not a distributed scheduler. This design prevents application-process downtime during updates on one host, but it cannot tolerate failure of the host, Docker daemon, single NGINX instance, or host network. Cross-host availability will require an orchestrator or an external load balancer.

Database migrations must follow expand/migrate/contract so adjacent application versions can run concurrently. WebSockets, long polling, and unusually long requests must finish within the grace period.
