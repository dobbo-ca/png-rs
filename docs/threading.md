# Threading

Inflate is the only stage that gains from threads. Unfiltering and the Adam7 merge run on the
calling thread.

## When the parallel path runs

It is tried after the chunks before the first IDAT are read and the row buffers are booked. All
of these must hold:

- `max_threads > 1` (0 is treated as 1).
- The IDAT chunks that follow each other from the first one are all in the input, and their data
  adds up to at least 4 MiB (`PAR_MIN`; the hidden `png::__private::decode_threaded_min` takes
  another threshold, for the fuzz target `threads`).
- The concatenated IDAT data plus the whole filtered image fit in what is left of `max_alloc`.

Then the decoder:

1. Books both buffers (`cat`, `filtered`) in the ledger and copies the data of the IDAT run into
   `cat`, checking the CRC of each chunk. Only the IDAT run is scanned, not the chunks after it.
2. Inflates `cat` into `filtered` with `deflate::zlib::decode_parallel_into(.., max_threads, ..)`,
   with what is left of `max_alloc` as deflate-rs's budget.
3. Checks that the inflated length is exactly the length the image needs and that the zlib
   stream ends where the IDAT data ends (the last four bytes are the Adler-32 of the output, and
   nothing else in the data looks like it).
4. Unfilters serially into the output.
5. Continues at the chunk after the IDAT run: the chunks up to IEND are read by the serial chunk
   loop, so text and eXIf after the image are kept as in the serial path.

On success the two buffers and their booking stay until the call ends, so what is read after
the image sees the rest of the budget only.

## Fallback

If any step fails (a CRC, a deflate error of any kind including its own `AllocLimit`, a length or
trailer mismatch, a bad filter byte), the decoder frees `cat` and `filtered`, gives their booking
back, and runs the serial streaming path from the first IDAT chunk, which gives the result and
the error. A cancel is the one error that is returned at once.

So output and errors are the serial ones by construction, for every thread count. A memory limit
may depend on the thread count: the threaded path needs more. In the Go profile that includes a
text or eXIf chunk after the image data: it is dropped, not failed, when it no longer fits
(docs/profiles.md).

## What it does not do

- `max_threads = 1` never spawns a thread.
- No thread pool, no global state. Threads belong to one call.
- The thread count of one call, the calling thread included, never goes above `max_threads`.
- The memory of all threads is charged to the one `max_alloc` budget.

## Tests

Output equal for `max_threads` 1, 2 and the default; a thread counter checks the peak against
`max_threads`; parity and the fuzz target `threads` compare 1 against 4.
