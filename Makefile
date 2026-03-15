.DEFAULT_GOAL := help

.PHONY: help build test test-unit test-integration test-e2e check fmt lint clean gui-setup gui-dev gui-build

help:
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  %-20s %s\n", $$1, $$2}'

build: ## Build all crates
	cargo build --workspace

release: ## Build all crates in release mode
	cargo build --workspace --release

test: ## Run all tests (unit + integration)
	cargo test --workspace

test-unit: ## Run unit tests only
	cargo test --workspace --lib

test-integration: ## Run integration tests only (CLI black-box tests)
	cargo test -p kondi --test cli

test-e2e: ## Run end-to-end tests (spawns real daemon; run make build first)
	cargo build --workspace && cargo test -p kondi --test e2e

check: ## Check all crates compile without building
	cargo check --workspace

fmt: ## Format all code
	cargo fmt --all

fmt-check: ## Check formatting without modifying files
	cargo fmt --all -- --check

lint: ## Run Clippy lints
	cargo clippy --workspace --all-targets -- -D warnings

clean: ## Remove build artifacts
	cargo clean

gui-setup: ## Install GUI frontend dependencies (first time)
	cd crates/gui && pnpm install

gui-dev: ## Start Tauri dev server with hot-reload
	cd crates/gui && pnpm tauri dev

gui-build: ## Build Tauri app for production
	cd crates/gui && pnpm tauri build
