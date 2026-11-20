import importlib.util
from pathlib import Path
import unittest


spec = importlib.util.spec_from_file_location(
    "kokoro_server", Path(__file__).with_name("kokoro_server.py")
)
server = importlib.util.module_from_spec(spec)
spec.loader.exec_module(server)


class ResampleTest(unittest.TestCase):
    def test_worker_emits_device_rate_pcm(self):
        rate, samples = server.resample_to_device_rate([0.0, 1.0, 0.0], 24_000)
        self.assertEqual(rate, 16_000)
        self.assertEqual(len(samples), 2)
        self.assertAlmostEqual(samples[0], 0.0)
        self.assertAlmostEqual(samples[1], 0.5)


if __name__ == "__main__":
    unittest.main()
