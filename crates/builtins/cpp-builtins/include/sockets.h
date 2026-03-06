// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception
#ifndef SOCKETS_H
#define SOCKETS_H

#include "host_api.h"

namespace host_api {

class TCPSocket : public Resource {
protected:
  TCPSocket(std::unique_ptr<HandleState> state);

public:
  using AddressIPV4 = std::tuple<uint8_t, uint8_t, uint8_t, uint8_t>;
  using Port = uint16_t;

  enum IPAddressFamily { IPV4, IPV6 };

  TCPSocket() = delete;

  static TCPSocket *make(IPAddressFamily address_family);
  ~TCPSocket() override = default;

  bool connect(AddressIPV4 address, Port port);
  void close();
  bool send(HostString chunk);
  HostString receive(uint32_t chunk_size);
};

} // namespace host_api

#endif // SOCKETS_H
