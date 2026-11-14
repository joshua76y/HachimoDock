"""The P4 v3 PlatformIO workaround must not alter unrelated toolchains."""
import runpy
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "scripts/platformio_fix_p4_v3_cc.py"


class Environment:
    def __init__(self, family, compiler):
        self.family = family
        self.compiler = compiler

    def GetProjectOption(self, name, default):
        assert name == "custom_p4_chip_family"
        return self.family or default

    def subst(self, value):
        assert value == "$CC"
        return self.compiler

    def Replace(self, **values):
        self.compiler = values["CC"]

    def PioPlatform(self):
        return self

    def get_package_dir(self, name):
        assert name == "toolchain-riscv32-esp"
        return "/opt/esp"


def run(family, compiler):
    env = Environment(family, compiler)
    runpy.run_path(str(SCRIPT), init_globals={"env": env, "Import": lambda _: None})
    return env.compiler


def test_v3_riscv_absolute_compiler_is_normalized():
    assert run("v3", "/opt/esp/bin/riscv32-esp-elf-gcc") == "riscv32-esp-elf-gcc"


def test_other_targets_and_custom_compilers_are_unchanged():
    assert run("v1", "/opt/esp/bin/riscv32-esp-elf-gcc") == "/opt/esp/bin/riscv32-esp-elf-gcc"
    assert run("v3", "/opt/custom/bin/my-gcc") == "/opt/custom/bin/my-gcc"
    assert run("v3", "/opt/custom/bin/riscv32-esp-elf-gcc") == "/opt/custom/bin/riscv32-esp-elf-gcc"
    assert run("v3", "riscv32-esp-elf-gcc") == "riscv32-esp-elf-gcc"
