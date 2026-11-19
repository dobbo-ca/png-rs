/*
 * pngoracle: CI-only libpng oracle for png-rs parity tests (never linked into the crate).
 *
 *   build: gcc -O2 -Wall -Wextra -Werror -o /usr/local/bin/pngoracle pngoracle.c -lpng16
 *          (Debian bookworm, gcc 12, libpng 1.6.39; C99, libpng + libc only)
 *
 *   pngoracle <in-dir> <out-dir>
 *       Walks <in-dir> recursively; inputs are regular files whose name ends in ".png" or
 *       contains ".mut". Output base name = path relative to <in-dir> with '/' -> "__".
 *       Per input writes <name>.native, <name>.eight, <name>.meta, or <name>.err ("error\n")
 *       when libpng fails in either read (png_error / longjmp).
 *   pngoracle bench <dir> <reps>
 *       Per input decodes to the Eight layout <reps> times and prints TSV
 *       "name\tbytes\tns" (bytes = decoded size, ns = mean nanoseconds per decode,
 *       CLOCK_MONOTONIC, file already in memory). Inputs libpng rejects are skipped.
 *
 * Output formats (see docs/design.md):
 *   .native  rows as libpng returns them with no transforms except
 *            png_set_interlace_handling (Adam7 merged, sub-byte packed, 16-bit big-endian).
 *   .eight   second, independent read with png_set_expand + png_set_strip_16 +
 *            png_set_interlace_handling.
 *   .meta    from the first read: size/depth/color/interlace, plte, trns lines. tRNS is the
 *            raw chunk data as libpng kept it (palette: alpha array; gray: 2-byte BE value;
 *            RGB: three 2-byte BE values). If libpng dropped the chunk there is no line.
 *
 * Behaviour choices:
 *   - Input is read into memory and fed through png_set_read_fn; classic API, setjmp per read.
 *   - libpng's default CRC handling and benign-error behaviour are kept untouched; that is
 *     what png-rs Profile::Libpng copies.
 *   - Each read ends with png_read_end(png, end_info), so chunks after IDAT are read and
 *     checked exactly as a real reader that consumes the whole stream would; an error there
 *     makes the file an error. This is what Profile::Libpng must match for trailing chunks.
 *   - png_set_user_limits(0x7fffffff, 0x7fffffff); png_set_chunk_cache_max(0) and
 *     png_set_chunk_malloc_max(0): in libpng 1.6 a value of 0 means unlimited.
 *   - Decoded size (rowbytes * height) above 2 GiB is treated as an error.
 *   - Rows are read with png_read_row per pass, so no per-row pointer array is needed.
 */
#define _XOPEN_SOURCE 700
#include <ftw.h>
#include <png.h>
#include <setjmp.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/stat.h>
#include <string.h>
#include <time.h>

#define MAX_DECODED ((size_t)1 << 31)

struct mem {
    const uint8_t *data;
    size_t size, pos;
};

struct ctx {
    png_structp png;
    png_infop info, end;
    uint8_t *pix; /* decoded bytes */
    size_t len;
    char meta[4096];
    int eight, want_meta;
    struct mem m;
};

static void on_error(png_structp png, png_const_charp msg)
{
    (void)msg;
    png_longjmp(png, 1);
}

static void on_warn(png_structp png, png_const_charp msg)
{
    (void)png;
    (void)msg;
}

static void read_mem(png_structp png, png_bytep out, size_t n)
{
    struct mem *m = (struct mem *)png_get_io_ptr(png);
    if (n > m->size - m->pos)
        png_error(png, "eof");
    memcpy(out, m->data + m->pos, n);
    m->pos += n;
}

static void hex(char *dst, const uint8_t *src, size_t n)
{
    static const char d[] = "0123456789abcdef";
    for (size_t i = 0; i < n; i++) {
        dst[2 * i] = d[src[i] >> 4];
        dst[2 * i + 1] = d[src[i] & 15];
    }
    dst[2 * n] = 0;
}

/* Fills c->meta from the info struct (libpng's view). */
static void build_meta(struct ctx *c)
{
    png_uint_32 w, h;
    int depth, color, interlace;
    png_get_IHDR(c->png, c->info, &w, &h, &depth, &color, &interlace, NULL, NULL);
    int n = snprintf(c->meta, sizeof c->meta, "size %u %u\ndepth %d\ncolor %d\ninterlace %d\n",
                     (unsigned)w, (unsigned)h, depth, color, interlace != PNG_INTERLACE_NONE);
    char *p = c->meta + n;

    png_colorp pal;
    int npal;
    if (png_get_PLTE(c->png, c->info, &pal, &npal) && npal > 0 && npal <= 256) {
        uint8_t raw[768];
        for (int i = 0; i < npal; i++) {
            raw[3 * i] = pal[i].red;
            raw[3 * i + 1] = pal[i].green;
            raw[3 * i + 2] = pal[i].blue;
        }
        p += sprintf(p, "plte %d ", npal);
        hex(p, raw, (size_t)npal * 3);
        p += strlen(p);
        *p++ = '\n';
    }

    png_bytep alpha;
    int ntrans;
    png_color_16p tc;
    if (png_get_tRNS(c->png, c->info, &alpha, &ntrans, &tc)) {
        uint8_t raw[256];
        size_t rn = 0;
        if (color == PNG_COLOR_TYPE_PALETTE) {
            if (alpha && ntrans > 0 && ntrans <= 256) {
                memcpy(raw, alpha, (size_t)ntrans);
                rn = (size_t)ntrans;
            }
        } else if (tc) {
            uint16_t v[3];
            size_t nv = 1;
            if (color == PNG_COLOR_TYPE_RGB) {
                v[0] = tc->red;
                v[1] = tc->green;
                v[2] = tc->blue;
                nv = 3;
            } else {
                v[0] = tc->gray;
            }
            for (size_t i = 0; i < nv; i++) {
                raw[2 * i] = (uint8_t)(v[i] >> 8);
                raw[2 * i + 1] = (uint8_t)v[i];
            }
            rn = 2 * nv;
        }
        if (rn) {
            p += sprintf(p, "trns ");
            hex(p, raw, rn);
            p += strlen(p);
            *p++ = '\n';
            *p = 0;
        }
    }
}

/* Decodes data into c->pix/c->len. eight: expand + strip_16. Returns 0 ok, -1 error. */
static int decode(struct ctx *c, const uint8_t *data, size_t size, int eight, int want_meta)
{
    memset(c, 0, sizeof *c);
    c->m.data = data;
    c->m.size = size;
    c->eight = eight;
    c->want_meta = want_meta;
    c->png = png_create_read_struct(PNG_LIBPNG_VER_STRING, NULL, on_error, on_warn);
    if (!c->png)
        return -1;
    c->info = png_create_info_struct(c->png);
    c->end = png_create_info_struct(c->png);
    if (!c->info || !c->end || setjmp(png_jmpbuf(c->png))) {
        png_destroy_read_struct(&c->png, &c->info, &c->end);
        free(c->pix);
        c->pix = NULL;
        return -1;
    }
    png_set_read_fn(c->png, &c->m, read_mem);
    png_set_user_limits(c->png, 0x7fffffff, 0x7fffffff);
    png_set_chunk_cache_max(c->png, 0);  /* 0 = unlimited in libpng 1.6 */
    png_set_chunk_malloc_max(c->png, 0); /* 0 = unlimited in libpng 1.6 */
    png_read_info(c->png, c->info);
    if (c->eight) {
        png_set_expand(c->png);
        png_set_strip_16(c->png);
    }
    int passes = png_set_interlace_handling(c->png);
    png_read_update_info(c->png, c->info);

    size_t rb = png_get_rowbytes(c->png, c->info);
    size_t h = png_get_image_height(c->png, c->info);
    if (rb == 0 || h == 0 || h > MAX_DECODED / rb)
        png_error(c->png, "too big");
    c->len = rb * h;
    c->pix = malloc(c->len);
    if (!c->pix)
        png_error(c->png, "oom");
    for (int pass = 0; pass < passes; pass++)
        for (size_t y = 0; y < h; y++)
            png_read_row(c->png, c->pix + y * rb, NULL);
    png_read_end(c->png, c->end);
    if (!c->eight) {
        /* The unused low bits of the last byte of a sub-byte row are unspecified: zero them
         * (png-rs Format::Native defines them as zero). */
        size_t w = png_get_image_width(c->png, c->info);
        unsigned depth = png_get_bit_depth(c->png, c->info);
        unsigned rem = (unsigned)((w * depth) % 8);
        if (depth < 8 && rem != 0)
            for (size_t y = 0; y < h; y++)
                c->pix[y * rb + rb - 1] &= (uint8_t)(0xff << (8 - rem));
    }
    if (c->want_meta)
        build_meta(c);
    png_destroy_read_struct(&c->png, &c->info, &c->end);
    return 0;
}

static uint8_t *slurp(const char *path, size_t *n)
{
    FILE *f = fopen(path, "rb");
    if (!f)
        return NULL;
    size_t cap = 1 << 16, len = 0;
    uint8_t *b = malloc(cap);
    while (b) {
        size_t r = fread(b + len, 1, cap - len, f);
        len += r;
        if (len < cap)
            break;
        if (cap > MAX_DECODED) {
            free(b);
            b = NULL;
            break;
        }
        uint8_t *nb = realloc(b, cap * 2);
        if (!nb) {
            free(b);
            b = NULL;
            break;
        }
        b = nb;
        cap *= 2;
    }
    fclose(f);
    *n = len;
    return b;
}

static int write_file(const char *dir, const char *name, const char *ext, const void *p, size_t n)
{
    char path[4096];
    if (snprintf(path, sizeof path, "%s/%s.%s", dir, name, ext) >= (int)sizeof path)
        return -1;
    FILE *f = fopen(path, "wb");
    if (!f)
        return -1;
    int ok = fwrite(p, 1, n, f) == n;
    return (fclose(f) == 0 && ok) ? 0 : -1;
}

static int bench_mode;
static unsigned long reps;
static const char *in_root, *out_dir;
static int failures;

static int is_input(const char *p)
{
    const char *base = strrchr(p, '/');
    base = base ? base + 1 : p;
    size_t n = strlen(base);
    return (n >= 4 && !strcmp(base + n - 4, ".png")) || strstr(base, ".mut");
}

static int visit(const char *path, const struct stat *sb, int type, struct FTW *ftw)
{
    (void)sb;
    (void)ftw;
    if (type != FTW_F || !is_input(path))
        return 0;
    const char *rel = path + strlen(in_root);
    while (*rel == '/')
        rel++;
    char out[8192];
    size_t o = 0;
    for (const char *s = rel; *s && o < sizeof out - 2; s++) {
        if (*s == '/')
            out[o++] = '_', out[o++] = '_';
        else
            out[o++] = *s;
    }
    out[o] = 0;

    size_t n = 0;
    uint8_t *data = slurp(path, &n);
    if (!data) {
        fprintf(stderr, "cannot read %s\n", path);
        failures++;
        return 0;
    }
    static struct ctx c; /* large; keep off the stack */
    if (bench_mode) {
        struct timespec a, b;
        int ok = 1;
        clock_gettime(CLOCK_MONOTONIC, &a);
        for (unsigned long i = 0; i < reps && ok; i++) {
            ok = decode(&c, data, n, 1, 0) == 0;
            if (ok)
                free(c.pix);
        }
        clock_gettime(CLOCK_MONOTONIC, &b);
        if (ok && reps) {
            double ns = ((double)(b.tv_sec - a.tv_sec) * 1e9 + (double)(b.tv_nsec - a.tv_nsec)) /
                        (double)reps;
            printf("%s\t%zu\t%.0f\n", out, c.len, ns);
        }
    } else {
        int bad = 0;
        if (decode(&c, data, n, 0, 1) == 0) {
            bad |= write_file(out_dir, out, "native", c.pix, c.len);
            bad |= write_file(out_dir, out, "meta", c.meta, strlen(c.meta));
            free(c.pix);
            if (decode(&c, data, n, 1, 0) == 0) {
                bad |= write_file(out_dir, out, "eight", c.pix, c.len);
                free(c.pix);
            } else {
                bad = -2;
            }
        } else {
            bad = -2;
        }
        if (bad == -2) {
            /* either read failed: only .err remains */
            static const char *ext[] = {"native", "eight", "meta"};
            for (int i = 0; i < 3; i++) {
                char p[4096];
                if (snprintf(p, sizeof p, "%s/%s.%s", out_dir, out, ext[i]) < (int)sizeof p)
                    remove(p);
            }
            bad = write_file(out_dir, out, "err", "error\n", 6);
        }
        if (bad)
            failures++;
    }
    free(data);
    return 0;
}

int main(int argc, char **argv)
{
    if (argc == 4 && !strcmp(argv[1], "bench")) {
        bench_mode = 1;
        in_root = argv[2];
        reps = strtoul(argv[3], NULL, 10);
    } else if (argc == 3) {
        in_root = argv[1];
        out_dir = argv[2];
    } else {
        fprintf(stderr, "usage: pngoracle <in-dir> <out-dir> | pngoracle bench <dir> <reps>\n");
        return 2;
    }
    if (nftw(in_root, visit, 32, FTW_PHYS) != 0) {
        fprintf(stderr, "walk failed: %s\n", in_root);
        return 1;
    }
    return failures ? 1 : 0;
}
