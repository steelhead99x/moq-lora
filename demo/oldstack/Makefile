.PHONY: demo quick test
demo:
	./run.sh
quick:
	./run.sh --quick --results runs/quick-results
test:
	cargo test -q -p moq-lora-oldstack-demo
	.venv/bin/python -m pytest -q tests
