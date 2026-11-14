"""Work around pioarduino's ESP-IDF objdump path handling for P4 v3.

The platform derives objdump by joining its toolchain directory to $CC.  Some
installations supply $CC as an absolute path, which breaks that derivation.
The pre-build toolchain script already puts the matching binaries on PATH.
Run after the platform configures its tools; leave other targets/tools alone.
"""

Import("env")

import os

compiler = env.subst("$CC")
name = os.path.basename(compiler)
toolchain = env.PioPlatform().get_package_dir("toolchain-riscv32-esp")
if (env.GetProjectOption("custom_p4_chip_family", "v1") == "v3"
        and os.path.isabs(compiler)
        and toolchain
        and os.path.realpath(os.path.dirname(compiler)) == os.path.realpath(os.path.join(toolchain, "bin"))
        and name in ("riscv32-esp-elf-gcc", "riscv32-esp-elf-gcc.exe")):
    env.Replace(CC=name)
