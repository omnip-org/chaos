FROM postgres:18.4-trixie

ARG PIGSTY_ASC_SHA256=17b9c7727c3a4d3b77463299ade471b28a8ba97da263e6bbb02986a578de0882
ARG PG_CRON_PACKAGE_VERSION=1.6.7-3.pgdg13+1
ARG PGMQ_PACKAGE_VERSION=1.12.0-1PIGSTY~trixie
ARG PG_PARTMAN_PACKAGE_VERSION=5.5.0-2PIGSTY~trixie

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && install -d -m 0755 /etc/apt/keyrings \
    && curl -fsSL https://repo.pigsty.io/key \
        -o /etc/apt/keyrings/pigsty.asc \
    && printf '%s  %s\n' \
        "${PIGSTY_ASC_SHA256}" /etc/apt/keyrings/pigsty.asc \
        > /tmp/pigsty-key.sha256 \
    && sha256sum -c /tmp/pigsty-key.sha256 \
    && printf '%s\n' \
        'deb [signed-by=/etc/apt/keyrings/pigsty.asc] https://repo.pigsty.io/apt/pgsql/trixie trixie main' \
        > /etc/apt/sources.list.d/pigsty.list \
    && apt-get update \
    && apt-get install -y --no-install-recommends \
        postgresql-18-cron="${PG_CRON_PACKAGE_VERSION}" \
        postgresql-18-pgmq="${PGMQ_PACKAGE_VERSION}" \
        postgresql-18-partman="${PG_PARTMAN_PACKAGE_VERSION}" \
    && test -f /usr/share/postgresql/18/extension/pg_cron.control \
    && test -f /usr/share/postgresql/18/extension/pgmq.control \
    && test -f /usr/share/postgresql/18/extension/pg_partman.control \
    && rm -f /tmp/pigsty-key.sha256 \
    && rm -rf /var/lib/apt/lists/*
