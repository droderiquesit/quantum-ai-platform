"""qip-scraper: an operator-run tool that turns web pages into reviewable proposals.

It fetches each page itself, hands the text to ScrapeGraphAI's
SmartScraperGraph with Claude as the model, and writes a JSON proposal in
which every extracted value is marked as found, or not found, verbatim in the
bytes that were fetched. Nothing in the platform reads what it writes; a
person does.

Nothing in this package imports ScrapeGraphAI at module level. The library
reads its telemetry switch and writes a config file into the home directory
when it is first imported, so the import happens only inside
``guard.prepare()``, after the environment has been made safe for it.
"""

TOOL_NAME = "qip-scraper"
__version__ = "0.1.0"
