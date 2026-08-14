# ADR 0002: Zero-Downtime Blue/Green Updates with Docker Compose

- Status: Accepted
- Date: 2026-08-14

## Decision

Kubernetes is not part of the current deployment target. Docker Compose runs `api-blue` and `api-green` concurrently, with Caddy providing a stable entry point and active health checks. A release replaces blue and green sequentially, waiting for each new instance to become ready before replacing the other.

SIGTERM starts this sequence: mark the instance as draining, fail readiness, wait five seconds for Caddy to remove the instance, close the listener, and finish in-flight requests. Compose provides a 45-second `stop_grace_period`.

## Limitations

Docker Compose is not a distributed scheduler. This design prevents application-process downtime during updates on one host, but it cannot tolerate failure of the host, Docker daemon, single Caddy instance, or host network. Cross-host availability will require an orchestrator or an external load balancer.

Database migrations must follow expand/migrate/contract so adjacent application versions can run concurrently. WebSockets, long polling, and unusually long requests must finish within the grace period.
