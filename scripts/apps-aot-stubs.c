// One Rust object holds the front-end exports and code the front end never calls.
// GNU ld cannot drop those functions, so their BoringSSL and Bun I/O symbols need definitions.
#include <stdlib.h>

#define STUB(name) \
  void name(void) { abort(); }

STUB(ASN1_STRING_to_UTF8)
STUB(__bun_run_io)
STUB(GENERAL_NAME_free)
STUB(i2d_SSL_SESSION)
STUB(OPENSSL_free)
STUB(sk_num)
STUB(sk_pop_free_ex)
STUB(sk_value)
STUB(SSL_SESSION_free)
STUB(SSL_SESSION_up_ref)
STUB(X509_get_ext)
STUB(X509_get_ext_by_NID)
STUB(X509_get_subject_name)
STUB(X509_NAME_ENTRY_get_data)
STUB(X509_NAME_get_entry)
STUB(X509_NAME_get_index_by_NID)
STUB(X509V3_EXT_d2i)
STUB(X509V3_EXT_get)
STUB(X509V3_EXT_get_nid)
