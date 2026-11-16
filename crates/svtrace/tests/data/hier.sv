// Hierarchy: how types and values are spelled, type parameters, interface ports, uninstantiated blocks.
package hp;
    typedef enum logic [1:0] { A, B, C } mode_e;
    typedef struct packed { logic a; logic [3:0] b; } pair_t;
    typedef logic [7:0] byte_t;
    typedef byte_t alias_t;
    parameter int unsigned DEPTH = 4;
    parameter mode_e MODE = B;
endpackage

interface bus_if;
    logic [7:0] data;
    logic valid;
    modport src (output data, output valid);
    modport dst (input data, input valid);
endinterface

module child #(parameter type T = logic [3:0], parameter int W = 2, parameter bit FLAG = 1'b1) (
    input T x,
    bus_if.dst port
);
    logic [W-1:0] r;
endmodule

module hier_top (input logic clk, output hp::pair_t p_o);
    import hp::*;
    int unsigned count;
    integer i32;
    shortint signed tiny;
    byte_t bytes [0:3];
    logic [7:0] mem [3:0][1:0];
    logic q [$];
    real ratio;
    string name;
    mode_e mode;
    alias_t al;
    enum { IDLE, RUN } state;
    struct packed { logic x; logic y; } anon;
    parameter string S = "hi";
    localparam bit ONE = 1'b1;
    bus_if bus ();
    child #(.T(byte_t), .W(3)) u_child (.x(8'h0), .port(bus));
    for (genvar g = 1; g < 3; g++) begin : gl
        logic [g:0] v;
    end
    if (DEPTH == 5) begin : never
        logic z;
    end else begin : taken
        logic y;
    end
endmodule
