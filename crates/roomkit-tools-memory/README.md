# roomkit-tools-memory

Expose one `memory` tool over a host-selected namespace. Search with an empty query for the index, or supply words to search all note titles and bodies. Read by ID. Save creates a note under a new ID. To replace or forget a note, supply the revision observed on read or search.

`FileMemory` stores UTF-8 Markdown notes under a trusted, private root. A note starts with a level-one heading, a blank line and its body. The derived `MEMORY.md` lists titles and IDs. The host supplies the byte quota, including the index. Cooperating processes serialize mutations and check revisions under the same file lock. Independent hosts can implement `MemoryBackend` over another store. The host supplies permissions and decides how much output to present to a model.
