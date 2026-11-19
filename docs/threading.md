# Threading

Inflate is the only stage that gains from threads. Unfiltering and the Adam7 merge run on the
calling thread.

## When the parallel path runs

All of these must hold:

- `max_threads > 1`.
- The compressed IDAT data is at least 4 MiB.
- The whole filtered image plus the concatenated IDAT data fits in what is left of `max_alloc`.

Then the decoder:

1. Walks every chunk to IEND (CRC, order, lengths), without inflating.
2. If the walk finds nothing wrong, inflates the concatenated IDAT data with
   `deflate::zlib::decode_parallel_into(.., max_threads, ..)`.
3. Unfilters serially.

## Fallback

If the walk finds a problem, or the parallel inflate returns any error, or the inflated length is
not exactly the length the image needs (plus anything the active profile treats as an error), the
decoder runs the serial streaming path from the start and returns its result.

So output and errors are the serial ones by construction, for every thread count.

## What it does not do

- `max_threads = 1` never spawns a thread.
- No thread pool, no global state. Threads belong to one call.
- The thread count of one call, the calling thread included, never goes above `max_threads`.
- The memory of all threads is charged to the one `max_alloc` budget.

## Tests

Output equal for `max_threads` 1, 2 and the default; a thread counter checks the peak against
`max_threads`; parity and the fuzz target `threads` compare 1 against 4.
