# syntax=docker/dockerfile:1
#
# A6/C8 attestation tooling, layered ON TOP of the pinned monolith-builder
# digest. Layering (rather than rebuilding from scratch) preserves the exact
# toolchain the substrate run proved — D2 nightly, D3 float target, D7
# libunwind shim — so charter C11 still holds: the shipped image descends
# from the trusted, pinned base by construction.
# BASE_IMAGE defaults to the current pin and is overridden by
# rebuild-monolith-builder.yml with whatever ci/monolith-builder.image holds,
# so that file stays the single source of truth for the pin.
ARG BASE_IMAGE=ghcr.io/tuckermclean/monolith-builder@sha256:c8978fe3b56cf6ec1714b1b878997c7082cbf9786caa2b8cbcdbd986169c0daf
FROM ${BASE_IMAGE}

# Floors verified on crates.io 2026-09-19. These are floors, not hard pins:
# bump to whatever is current when the image is actually rebuilt.
ARG CARGO_AUDITABLE_VERSION=0.7.6
ARG CARGO_AUDIT_VERSION=0.22.2

RUN set -eux; \
    cargo install cargo-auditable --version "${CARGO_AUDITABLE_VERSION}" --locked; \
    cargo install cargo-audit --version "${CARGO_AUDIT_VERSION}" --locked \
      || cargo install cargo-audit --version "${CARGO_AUDIT_VERSION}" --locked \
           --no-default-features --features fix,vendored-openssl; \
    cargo auditable --version; \
    cargo audit --version
