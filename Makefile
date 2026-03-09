.DEFAULT_GOAL := help

.PHONY: help build test test-unit test-integration check fmt lint clean

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
