# Sumibi-win consolidated build and test workflow

.PHONY: all session-test build build-tip test test-all check fmt verify ci register-dev unregister-dev verify-com help

all: session-test

session-test:
	powershell -ExecutionPolicy Bypass -File tools/dev.ps1 session-test

build-tip:
	powershell -ExecutionPolicy Bypass -File tools/dev.ps1 build-tip

build:
	powershell -ExecutionPolicy Bypass -File tools/dev.ps1 build

test:
	powershell -ExecutionPolicy Bypass -File tools/dev.ps1 test

test-all:
	powershell -ExecutionPolicy Bypass -File tools/dev.ps1 test-all

check:
	powershell -ExecutionPolicy Bypass -File tools/dev.ps1 check

fmt:
	powershell -ExecutionPolicy Bypass -File tools/dev.ps1 fmt

verify:
	powershell -ExecutionPolicy Bypass -File tools/dev.ps1 verify

ci:
	powershell -ExecutionPolicy Bypass -File tools/dev.ps1 ci

register-dev:
	powershell -ExecutionPolicy Bypass -File tools/dev.ps1 register-dev

unregister-dev:
	powershell -ExecutionPolicy Bypass -File tools/dev.ps1 unregister-dev

verify-com:
	powershell -ExecutionPolicy Bypass -File tools/dev.ps1 verify-com

help:
	powershell -ExecutionPolicy Bypass -File tools/dev.ps1 help
