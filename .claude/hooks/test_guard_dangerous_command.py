#!/usr/bin/env python3
"""Unit tests for guard-dangerous-command.py.

Tests that dangerous commands are refused and safe ones are allowed.
Run with: python -m pytest test_guard_dangerous_command.py -v
"""

import json
import subprocess
import sys
from pathlib import Path


def run_guard(command: str) -> int:
    """Run the guard hook with a command and return the exit code."""
    hook_script = Path(__file__).parent / "guard-dangerous-command.py"
    payload = json.dumps({"tool_input": {"command": command}})
    result = subprocess.run(
        [sys.executable, str(hook_script)],
        input=payload,
        capture_output=True,
        text=True,
    )
    return result.returncode


def test_terraform_apply_prod_is_refused():
    """terraform apply targeting prod should be refused."""
    commands = [
        "terraform apply -var-file=infrastructure/environments/prod/terraform.tfvars",
        "terraform apply -chdir=infrastructure/environments/prod",
        "cd infrastructure/environments/prod && terraform apply",
        "terraform apply prod.tfplan",
    ]
    for cmd in commands:
        exit_code = run_guard(cmd)
        assert exit_code == 2, f"Expected refusal for: {cmd}"


def test_terraform_apply_nonprod_is_allowed():
    """terraform apply targeting non-prod environments should be allowed."""
    commands = [
        "terraform apply -var-file=infrastructure/environments/dev/terraform.tfvars",
        "terraform apply -var-file=infrastructure/environments/stage/terraform.tfvars",
        "terraform apply -var-file=infrastructure/environments/test/terraform.tfvars",
    ]
    for cmd in commands:
        exit_code = run_guard(cmd)
        assert exit_code == 0, f"Expected approval for: {cmd}"


def test_terraform_plan_prod_is_allowed():
    """terraform plan (without apply) should be allowed even targeting prod."""
    cmd = "terraform plan -var-file=infrastructure/environments/prod/terraform.tfvars"
    exit_code = run_guard(cmd)
    assert exit_code == 0, f"terraform plan should be allowed: {cmd}"


def test_force_push_is_refused():
    """git push --force should be refused."""
    exit_code = run_guard("git push --force")
    assert exit_code == 2


def test_heredoc_not_matched():
    """A heredoc containing dangerous text should not be refused."""
    cmd = """cat <<'EOF'
terraform apply prod
EOF
"""
    exit_code = run_guard(cmd)
    assert exit_code == 0, "Heredoc body should be stripped"


if __name__ == "__main__":
    test_terraform_apply_prod_is_refused()
    print("✓ terraform apply prod is refused")

    test_terraform_apply_nonprod_is_allowed()
    print("✓ terraform apply non-prod is allowed")

    test_terraform_plan_prod_is_allowed()
    print("✓ terraform plan prod is allowed")

    test_force_push_is_refused()
    print("✓ git push --force is refused")

    test_heredoc_not_matched()
    print("✓ heredoc bodies are not matched")

    print("\nAll tests passed!")
