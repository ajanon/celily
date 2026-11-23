"""The smallest run there is, and what it must leave behind: nothing."""


def test_true(celily):
    result = celily.run("--", "true")

    assert result.returncode == 0
    # stdout belongs to the command: celily's own output goes to stderr.
    assert result.stdout == ""
